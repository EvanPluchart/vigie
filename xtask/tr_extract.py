"""Lists the @tr strings of the Slint files, in order of first use (used to write the .po)."""
import glob
import json
import re
import sys

PATTERN = re.compile(r'@tr\(\s*"((?:[^"\\]|\\.)*)"')

ids = []
for path in sorted(glob.glob("crates/vigie-app/ui/*.slint")):
    with open(path, encoding="utf-8") as f:
        for m in PATTERN.finditer(f.read()):
            if m.group(1) not in ids:
                ids.append(m.group(1))
json.dump(ids, sys.stdout, ensure_ascii=False, indent=0)
