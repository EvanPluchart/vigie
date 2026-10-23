//! Checks that every interface string has its French translation (decision 0008).

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;

    /// The first string literal of every `@tr(...)`, escapes kept as written.
    fn tr_strings(source: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = source;
        while let Some(at) = rest.find("@tr(") {
            rest = rest[at + 4..].trim_start();
            let Some(body) = rest.strip_prefix('"') else {
                continue;
            };
            let mut text = String::new();
            let mut chars = body.char_indices();
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => {
                        text.push(c);
                        if let Some((_, next)) = chars.next() {
                            text.push(next);
                        }
                    }
                    '"' => {
                        rest = &body[i + 1..];
                        break;
                    }
                    _ => text.push(c),
                }
            }
            out.push(text);
        }
        out
    }

    /// The msgids of a .po file, escapes kept as written.
    fn po_ids(po: &str) -> HashSet<String> {
        po.lines()
            .filter_map(|l| l.strip_prefix("msgid \""))
            .filter_map(|l| l.strip_suffix('"'))
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn every_string_has_a_french_translation() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let po = std::fs::read_to_string(root.join("translations/fr/LC_MESSAGES/vigie.po"))
            .expect("the French translation");
        let ids = po_ids(&po);
        let mut missing = Vec::new();
        let mut count = 0;
        for entry in std::fs::read_dir(root.join("ui")).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "slint") {
                for text in tr_strings(&std::fs::read_to_string(&path).unwrap()) {
                    count += 1;
                    if !ids.contains(&text) {
                        missing.push(format!("{}: {text}", path.display()));
                    }
                }
            }
        }
        assert!(
            count > 200,
            "found only {count} strings: is the extraction broken?"
        );
        assert!(
            missing.is_empty(),
            "no French for (run `python xtask/tr_fr.py`):\n{}",
            missing.join("\n")
        );
    }
}
