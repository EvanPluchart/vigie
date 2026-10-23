//! GPU utilisation and dedicated memory on Windows, through the kernel graphics statistics
//! (D3DKMT), the source Task Manager's GPU figures also come from. No PDH: it would load
//! perflib into Vigie (decision 0005).

use windows_sys::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTERINFO, D3DKMT_ADAPTERREGISTRYINFO, D3DKMT_CLOSEADAPTER, D3DKMT_ENUMADAPTERS2,
    D3DKMT_QUERYADAPTERINFO, D3DKMT_QUERYSTATISTICS, D3DKMT_QUERYSTATISTICS_ADAPTER,
    D3DKMT_QUERYSTATISTICS_NODE, D3DKMT_QUERYSTATISTICS_SEGMENT, D3DKMTCloseAdapter,
    D3DKMTEnumAdapters2, D3DKMTQueryAdapterInfo, D3DKMTQueryStatistics,
    KMTQAITYPE_ADAPTERREGISTRYINFO,
};
use windows_sys::Win32::Foundation::LUID;

use crate::GpuCounters;

/// Software adapter Windows always lists; it is the CPU, not a GPU.
const BASIC_RENDER: &str = "Microsoft Basic Render Driver";

struct Adapter {
    luid: LUID,
    nodes: u32,
    segments: u32,
    name: String,
    /// Last raw running time and idle streak of each engine (see `Gpus::read`).
    engines: Vec<Engine>,
}

#[derive(Clone, Copy, Default)]
struct Engine {
    last_ns: u64,
    idle_reads: u32,
}

/// Each engine query is a driver round trip: about 0.4 ms of CPU on AMD drivers, 8 ms for two
/// GPUs with 19 engines. Engines idle for `IDLE_AFTER` reads are skipped and checked again
/// every `RECHECK_EVERY` reads; most of them (video, copy) idle most of the time.
#[derive(Default)]
pub(crate) struct Gpus {
    adapters: Vec<Adapter>,
    reads: u32,
}

const IDLE_AFTER: u32 = 10;
const RECHECK_EVERY: u32 = 30;

impl Gpus {
    /// Finds the hardware adapters once; GPUs do not come and go while Vigie runs.
    pub(crate) fn discover() -> Self {
        let mut list = D3DKMT_ENUMADAPTERS2::default();
        // SAFETY: with a null array, the call only reports how many adapters there are.
        if unsafe { D3DKMTEnumAdapters2(&mut list) } != 0 || list.NumAdapters == 0 {
            return Self::default();
        }
        let mut infos = vec![
            // SAFETY: an all-zero D3DKMT_ADAPTERINFO is a valid value.
            unsafe { std::mem::zeroed::<D3DKMT_ADAPTERINFO>() };
            list.NumAdapters as usize
        ];
        list.pAdapters = infos.as_mut_ptr();
        // SAFETY: `infos` holds NumAdapters entries for the call to fill.
        if unsafe { D3DKMTEnumAdapters2(&mut list) } != 0 {
            return Self::default();
        }
        let mut adapters = Vec::new();
        for info in &infos[..list.NumAdapters as usize] {
            let name = adapter_name(info.hAdapter);
            // SAFETY: EnumAdapters2 opened this handle for us; closed exactly once.
            unsafe {
                D3DKMTCloseAdapter(&D3DKMT_CLOSEADAPTER {
                    hAdapter: info.hAdapter,
                })
            };
            if name == BASIC_RENDER {
                continue;
            }
            let mut query = D3DKMT_QUERYSTATISTICS {
                Type: D3DKMT_QUERYSTATISTICS_ADAPTER,
                AdapterLuid: info.AdapterLuid,
                ..D3DKMT_QUERYSTATISTICS::default()
            };
            // SAFETY: `query` is a valid statistics request. The binding says `const`, but the
            // kernel writes the result into it: the pointer comes from a mutable borrow.
            if unsafe { D3DKMTQueryStatistics((&raw mut query).cast_const()) } != 0 {
                continue;
            }
            // SAFETY: an adapter query fills the AdapterInformation member of the result.
            let result = unsafe { query.QueryResult.AdapterInformation };
            if result.NodeCount == 0 {
                continue;
            }
            // Virtual adapters (remote desktop, display capture drivers) have no registry name
            // and no memory of their own: not a GPU the user owns.
            if name.is_empty() {
                continue;
            }
            adapters.push(Adapter {
                luid: info.AdapterLuid,
                nodes: result.NodeCount,
                segments: result.NbSegments,
                name,
                engines: vec![Engine::default(); result.NodeCount as usize],
            });
        }
        Self { adapters, reads: 0 }
    }

    pub(crate) fn names(&self) -> Vec<String> {
        self.adapters.iter().map(|a| a.name.clone()).collect()
    }

    /// Engine running times and dedicated memory of each adapter. Reuses `out`'s buffers.
    pub(crate) fn read(&mut self, out: &mut Vec<GpuCounters>) {
        let recheck = self.reads.is_multiple_of(RECHECK_EVERY);
        self.reads = self.reads.wrapping_add(1);
        out.resize_with(self.adapters.len(), GpuCounters::default);
        for (i, (adapter, counters)) in self.adapters.iter_mut().zip(out.iter_mut()).enumerate() {
            counters.index = i as u32;
            counters.engines_ns.clear();
            for node in 0..adapter.nodes {
                let engine = &mut adapter.engines[node as usize];
                if engine.idle_reads >= IDLE_AFTER && !recheck {
                    // Skipped: unchanged counter, so the consumer sees it idle.
                    counters.engines_ns.push(engine.last_ns);
                    continue;
                }
                let mut query = D3DKMT_QUERYSTATISTICS {
                    Type: D3DKMT_QUERYSTATISTICS_NODE,
                    AdapterLuid: adapter.luid,
                    ..D3DKMT_QUERYSTATISTICS::default()
                };
                query.Anonymous.QueryNode.NodeId = node;
                // SAFETY: a valid node request; a failure leaves the engine at 0.
                let running =
                    if unsafe { D3DKMTQueryStatistics((&raw mut query).cast_const()) } == 0 {
                        // SAFETY: a node query fills the NodeInformation member.
                        unsafe {
                            query
                                .QueryResult
                                .NodeInformation
                                .GlobalInformation
                                .RunningTime
                        }
                    } else {
                        0
                    };
                // Some drivers report odd (even negative) values: keep the raw counter, wrapping,
                // and let the consumer drop deltas that make no sense.
                let now = (running as u64).wrapping_mul(100);
                engine.idle_reads = if now == engine.last_ns {
                    engine.idle_reads.saturating_add(1)
                } else {
                    0
                };
                engine.last_ns = now;
                counters.engines_ns.push(now);
            }
            // Dedicated memory: the non-aperture segments (aperture segments are shared RAM).
            counters.dedicated_used = 0;
            counters.dedicated_total = 0;
            for segment in 0..adapter.segments {
                let mut query = D3DKMT_QUERYSTATISTICS {
                    Type: D3DKMT_QUERYSTATISTICS_SEGMENT,
                    AdapterLuid: adapter.luid,
                    ..D3DKMT_QUERYSTATISTICS::default()
                };
                query.Anonymous.QuerySegment.SegmentId = segment;
                // SAFETY: a valid segment request.
                if unsafe { D3DKMTQueryStatistics((&raw mut query).cast_const()) } != 0 {
                    continue;
                }
                // SAFETY: a segment query fills the SegmentInformation member.
                let info = unsafe { query.QueryResult.SegmentInformation };
                if info.Aperture == 0 {
                    counters.dedicated_used += info.BytesResident;
                    counters.dedicated_total += info.CommitLimit;
                }
            }
        }
    }
}

fn adapter_name(handle: u32) -> String {
    let mut info = D3DKMT_ADAPTERREGISTRYINFO::default();
    let mut query = D3DKMT_QUERYADAPTERINFO {
        hAdapter: handle,
        Type: KMTQAITYPE_ADAPTERREGISTRYINFO,
        pPrivateDriverData: (&mut info as *mut D3DKMT_ADAPTERREGISTRYINFO).cast(),
        PrivateDriverDataSize: size_of::<D3DKMT_ADAPTERREGISTRYINFO>() as u32,
    };
    // SAFETY: `info` is valid for the size given; the handle is open.
    if unsafe { D3DKMTQueryAdapterInfo(&mut query) } != 0 {
        return String::new();
    }
    let end = info
        .AdapterString
        .iter()
        .position(|&u| u == 0)
        .unwrap_or(info.AdapterString.len());
    String::from_utf16_lossy(&info.AdapterString[..end])
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_and_memory_are_readable() {
        let mut gpus = Gpus::discover();
        // A test machine may have no hardware GPU (a VM): then there is simply nothing to read.
        let mut out = Vec::new();
        gpus.read(&mut out);
        assert_eq!(out.len(), gpus.adapters.len());
        for (gpu, adapter) in out.iter().zip(&gpus.adapters) {
            assert!(!adapter.name.is_empty());
            assert_eq!(gpu.engines_ns.len() as u32, adapter.nodes);
            assert!(gpu.dedicated_used <= gpu.dedicated_total || gpu.dedicated_total == 0);
        }
    }
}
