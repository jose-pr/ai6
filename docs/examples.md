# Examples

## Run a local Station

Build AI6 and its providers, then start a Station on the sample configuration in
`examples/config`: `ai6.cfg` (the bootstrap file, naming that directory as the configuration
provider), the family files `agents.toml`, `operations.toml`, `theaters.yaml` and
`station.json` in three formats, and `unex.cfg` and `OO.cfg` for Universal Exports and Outer
Office ([Configuration](configuration.md)):

```console
cargo build --workspace --all-features
cargo run -p ai6 --features cli -- serve --config ./examples/config/ai6.cfg
```

## Queue a Mission

Sign in through Universal Exports and submit a brief to an Operation with
`mission.create`. The HTTP request and response contract are documented in
[Missions and intake](missions.md#queue-a-mission-through-universal-exports). The
Outer Office's Operations view offers the same form.

For an Outer Office preview with sample data and no connection, follow
[`clients/outer-office/README.md`](https://github.com/jose-pr/ai6/blob/main/clients/outer-office/README.md)
and enable `OO_DEMO=true`.
