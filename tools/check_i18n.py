#!/usr/bin/env python3
"""Compares the languages in app/assets/i18n with en.json."""
import glob, json, os, re, sys
d = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "app", "assets", "i18n")
en = json.load(open(f"{d}/en.json", encoding="utf8"))
ph = lambda s: sorted(re.findall(r"\{[a-z]\}", s))
bad = 0
files = sorted(glob.glob(f"{d}/*.json"))
for f in files:
    code = os.path.basename(f)[:-5]
    m = json.load(open(f, encoding="utf8"))
    for k in en.keys() - m.keys():
        print(code, "missing:", k); bad += 1
    for k in m.keys() - en.keys():
        print(code, "extra:", k); bad += 1
    for k in en.keys() & m.keys():
        if ph(en[k]) != ph(m[k]):
            print(code, k, "placeholder mismatch:", m[k]); bad += 1
print(f"{len(en)} keys, {len(files)} languages, {bad} problems")
sys.exit(1 if bad else 0)
