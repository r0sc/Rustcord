# Releasing: the update signing key

The release steps are in the README, under Development → Releasing. This file covers the
Ed25519 key that signs the update feed (`latest.json.sig`).

## Creating the key

Make the key once with OpenSSL 3:

```bash
openssl genpkey -algorithm ed25519 -out update-signing.pem
openssl pkey -in update-signing.pem -pubout -outform DER | tail -c 32 | base64 > assets/update-signing.pub
```

Commit `assets/update-signing.pub`. Paste the whole `update-signing.pem` into the repository
secret `RUSTCORD_UPDATE_SIGNING_KEY` (Settings → Secrets and variables → Actions), keep an
offline backup, and delete the file. Whoever has the private key can ship code to every
installed copy.

## Rotating the key

Release one version whose `assets/update-signing.pub` holds the new key, still signed with the
old key. Sign every release after it with the new key. Copies older than that release trust
only the old key, so they must update through it.

## Lost or leaked key

Without a backup, installed copies can no longer update themselves, since they accept only
feeds signed with the key they carry. Make a new key as above, commit its public half, store the
private half as the secret and release. Then tell users in the release notes and the README to
install that release once by hand. From there on, updates work again.

If the key leaked, do the same, delete the old secret and check who has access to the
repository. The app only downloads from this repository's release downloads, so the key alone is
not enough to ship code.

## Hardening

Recommended GitHub settings:

- Move the secret into an environment `release` with a required reviewer, and add
  `environment: release` to the workflow's `release` job.
- Protect `v*` tags so that only the maintainer can push them.
