# Publisher keys for `scripts/pin_reverse.py`

`gpgv` checks the reverse-engineering downloads against these keyrings when they are pinned
(item5-spec.md WP9a). They are binary OpenPGP keyrings (`gpg --dearmor` of the published
keys). The app never reads them; it checks every download against the SHA-256 in
`reverse.lock.json`.

- `nodejs.gpg`: every key in https://github.com/nodejs/release-keys (`keys/*.asc`, active
  and retired releasers, 28 keys) at commit 481637f813e912c4aa3622d7964ab426c97b8e8d.
  Checks `SHASUMS256.txt.asc` of a Node.js release.
- `adoptium.gpg`: Adoptium's signing key from
  https://packages.adoptium.net/artifactory/api/gpg/key/public, primary key fingerprint
  `3B04D753C9050D9A5D343F39843C48A565F8F04B` ("Adoptium GPG Key (DEB/RPM Signing Key)
  <temurin-dev@eclipse.org>"). Checks the `.sig` of each Eclipse Temurin archive.

To refresh a keyring, fetch the keys from those sources, compare the fingerprints with the
publishers' own pages, and write the dearmored keys here.
