# coagent-native

The native library [coagent](https://github.com/jose-pr/coagent)'s Python supervisor
loads to use terminal plugins: **guatiao's C ABI with plugin loading, in one file.** It
adds no code of its own; point `GUATIAO_LIBRARY` at the built
`libcoagent_native.{so,dylib}` / `coagent_native.dll`.

## License

MPL-2.0.
