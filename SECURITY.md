# Security Policy

## Reporting a vulnerability
Please report suspected vulnerabilities privately to security@integrallis.com, or via GitHub's
private "Report a vulnerability" advisory feature on this repository. Do not open a public issue
for security reports. We aim to acknowledge within 3 business days.

## Scope and threat model
`pleat` is an approximate-membership filter. Note:
- **Not adversary-resistant by construction.** Keys are hashed with a fixed, non-cryptographic
  seed (xxh3). An adversary who can choose queried keys can inflate the false-positive rate.
  Do not use it as a security boundary against chosen-key attacks without an application-level
  keyed/randomized layer.
- **Deserialization is validated.** `from_bytes` rejects malformed, truncated, wrong-family,
  wrong-width, or corrupted buffers (checksum) and never triggers undefined behavior; report any
  input that panics or misbehaves as a bug.
- **Unsafe code is confined to bounds-checked prefetches.** There is one prefetch site for each
  filter family on x86_64. Both obtain the hinted address through `slice::get`, and Miri exercises
  the serialization and batch-query paths with prefetch disabled; report any soundness concern.
