# Examples

## Run a local Station

Build AI6 and its providers, then use the sample Operation and Agent roster:

```console
cargo build --workspace --all-features
cargo run -p ai6 --features cli -- serve --config-dir ./examples/config --plugin-path ./target/debug
```

## Queue a Mission

Sign in through Universal Exports and submit a brief to an Operation with
`mission.create`. The HTTP request and response contract are documented in
[Missions and intake](missions.md#queue-a-mission-through-universal-exports). The
Moneypenny app's Operations view offers the same form.

For a Moneypenny preview with sample data and no connection, follow
[`clients/moneypenny/README.md`](https://github.com/jose-pr/ai6/blob/main/clients/moneypenny/README.md)
and enable `MONEYPENNY_DEMO=true`.
