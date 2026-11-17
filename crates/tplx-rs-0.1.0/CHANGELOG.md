# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Engines `raw`, `envsubst` and `mustache`, rendered by name with `render(engine, text, &data, &options)` or by
  their own functions, and found by name or file suffix through `Engine`.
- `envsubst`: `$NAME` and `${NAME}` in the `gnu` (default), `a8m` and `drone` dialects of tplx's engine, with
  options for the pattern, the features and what an undefined name does (`strict`, the default as in tplx, `lenient` or `debug`); names match
  case-insensitively and nested data is flattened.
- `mustache`: the official Mustache specification's comments, delimiters, interpolation, inverted sections,
  partials (through a `Resolver`) and sections; `{{x}}` escapes HTML unless `MustacheOptions::escape` is `none`.
  Lambdas and inheritance are not supported.
- `Limits` on output bytes, nesting depth and iterations, and an `Error` that tells an unknown engine, a refused
  template, an undefined name and a reached limit apart.
- `no_std` with `alloc`; builds for `wasm32-unknown-unknown`.

[Unreleased]: https://github.com/jose-pr/tplx-rs/commits/main
