# Catalog Refresh test fixture

`test.pub`, `models.json` and `models.json.minisig` were made with a throwaway
Ed25519 key in minisign's prehashed format. The secret key was discarded. They
exist only so the Refresh path (signature check, then sequence check) can be
tested; the app never trusts this key.
