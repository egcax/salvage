# Contributing

- Engine changes must keep `tests/run_tests.py` and `tests/v2_tests.py` passing.
- The source disk must stay read-only. No exceptions.
- A chance score must never claim more than was measured.
- Translations: edit `app/assets/i18n/<language>.json`, keep `{placeholders}` as they are.
- UI strings go through `t('key')`; add the key to `en.json` first.
