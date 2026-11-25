# Verus verification

The pure logic of this crate carries machine-checked specifications, verified
with [Verus](https://github.com/verus-lang/verus). Specifications are written
inline in the Rust sources with `#[verus_spec(...)]` attributes and `verus!`
blocks. When the crate is built with the normal Rust toolchain, all
specification and proof code is erased; verification is a separate step.

## Tooling

Install a Verus binary release for your platform and either put it on `PATH`
or point `VERUS_BIN_DIR` at the extracted directory:

```sh
curl -sL -o verus.zip \
  https://github.com/verus-lang/verus/releases/latest/download/verus-<version>-<platform>.zip
unzip verus.zip
bash verus-*/macos_allow_gatekeeper.sh  # macOS only
```

## Running verification

```sh
scripts/verus-verify.sh          # verify the crate
cargo build                      # normal build: specs are erased
cargo test                       # normal tests: unaffected
```

The script requires `cargo-verus` (bundled in the Verus release) to find the
crate through Cargo, so verification always checks the same sources that
`cargo build` compiles.

## What is verified

| Module | Function | Property |
|---|---|---|
| (none yet) | | |

Add a row here when a new function gets a specification.
