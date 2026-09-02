A 2048-bit RSA key pair, generated once for `tests/oidc_flow.rs`.

It is a test fixture and nothing else: it signs tokens issued by the fake
provider inside that test, over a loopback socket, in a process that exits when
the test does. It authenticates nothing, and publishing it costs nothing.

`signing-key.modulus` is the same key's modulus as base64url, which is the `n`
of the JWK the fake provider serves. Kept beside the PEM rather than derived at
test time so the test needs no ASN.1 parser to check a signature it made.

Both files are committed, and the repository's blanket `*.pem` rule has a
negation naming this one path. That rule swallowed the key once already: the
crate compiled for everyone who had generated one locally and for nobody
else, and CI found it rather than a person. If you move or rename this file,
move the negation in `.gitignore` with it.
