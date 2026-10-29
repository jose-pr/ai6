# Examples

## Run a local Station

Build AI6 and its providers, then use the sample Operation and Agent roster:

```console
cargo build --workspace --all-features
cargo run -p ai6 --features cli -- serve --provider ./examples/config --plugin-path ./target/debug
```

## Queue a Mission

Sign in through Universal Exports and submit a brief to an Operation with
`mission.create`. The HTTP request and response contract are documented in
[Missions and intake](missions.md#queue-a-mission-through-universal-exports). The
Outer Office's Operations view offers the same form.

For an Outer Office preview with sample data and no connection, follow
[`clients/outer-office/README.md`](https://github.com/jose-pr/ai6/blob/main/clients/outer-office/README.md)
and enable `OO_DEMO=true`.
