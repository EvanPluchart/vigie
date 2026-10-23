"""Pack PNG files into one .ico (PNG-compressed entries). usage: ico.py out.ico a.png b.png ..."""
import struct, sys
out, pngs = sys.argv[1], sys.argv[2:]
data = [open(p, "rb").read() for p in pngs]
sizes = [struct.unpack(">II", d[16:24]) for d in data]
header = struct.pack("<HHH", 0, 1, len(data))
offset = 6 + 16 * len(data)
entries = b""
for (w, h), d in zip(sizes, data):
    entries += struct.pack("<BBBBHHII", w % 256, h % 256, 0, 0, 1, 32, len(d), offset)
    offset += len(d)
open(out, "wb").write(header + entries + b"".join(data))
print(out, sizes)
