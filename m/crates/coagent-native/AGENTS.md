# `coagent-native` — API header

A `cdylib` exporting guatiao's C ABI built with the `load` and `provider` features,
which is what the `guatiao` Python package calls. It exports nothing else. Load it with
`GUATIAO_LIBRARY=<file>` or `guatiao.use_library(<file>)`. Self-contained: it ships
with the crate. Source: <https://github.com/jose-pr/coagent>.
