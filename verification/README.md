# Verus verification

The pure logic of this crate carries machine-checked specifications, verified
with [Verus](https://github.com/verus-lang/verus). Specifications are written
inline in the Rust sources with `#[verus_spec(...)]` attributes and `verus!`
blocks. `src/proofs.rs` holds the shared spec definitions and proof lemmas
for the validation rules. When the crate is built with the normal Rust
toolchain, all specification and proof code is erased; verification is a
separate step.

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

Run `scripts/verus-verify.sh`; the expected result is `11 verified, 0 errors`.

| Module | Function | Property |
|---|---|---|
| `shell_quote` | `quote_chars` | Returns exactly the spec model `quoted_prefix` of its input: each char passes through, each `'` becomes `'\''`. |
| `validate` | `pane_split_findings` | Reports no finding if and only if the pane signatures satisfy `split_rules_ok`: the first pane is not a split, every `from` names a non-empty id declared on an earlier pane, and ids are non-empty and unique within the tab. |

The `validate` contract is the trust boundary the planner relies on: configs
that pass validation cannot make `plan_workspace` panic on split-source
resolution.

## Trusted computing base

- `shell_quote` itself is `#[verus_verify(external_body)]`: its body only runs the standard-library conversions `str::chars` and `String: FromIterator<char>`, for which vstd has no specifications. The quoting logic that can carry a quoting bug lives in the verified `quote_chars`.
- `validate_pane_split_sources` trusts the projection of `Pane` id/from fields into `PaneSig` tuples (a `.map` over the panes); all decisions are verified over the signatures.


