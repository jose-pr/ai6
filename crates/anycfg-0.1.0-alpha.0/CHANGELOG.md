# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and the project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- Replace the fixed format enum and hard-coded parser dependencies with a
  registered `Codec<V>` interface, callback adapters and configurable format
  names, suffixes, native markers and fallback selection.
- Decode directly into the consumer's value model, with no intermediate
  format-specific or JSON-compatible representation.
- Delegate `Replace`, `Shallow`, `Deep` merging and structural depth
  validation to a `MergeValues<V>` implementation or merge callback.
- Convert the final merged value to an application type through `load_as`, whose
  callback returns `Result<T, CodecError>`.
- `CodecError` is `#[non_exhaustive]`, is no longer `Copy`, and gains `message`. Build it with
  `CodecError::new().line(2).column(5).message("...")`.
- `Error::Parse` and `Error::Serialize` gain `message`. `Error::Conversion` becomes
  `Conversion { path, message }`.
- `Error::UnsupportedFormat` gains `registered`, and `Error::UnsupportedExtension` gains
  `extension` and `registered`.
- `Options` and `Detection` are `#[non_exhaustive]`; build `Options` through its methods.
- A `Codec` given to `Formats::register` must be `Send + Sync`, which makes `Formats` both.
- A name's leading dot does not start an extension: `.app` has none.

### Removed

- `Error::InvalidShebang`: a first line naming no format is `Error::UnsupportedFormat`.
- The 256-byte limits on a format line and on a marker: the input's byte limit bounds them.
- `DESIGN.md`, whose content is in the README and the guides.

### Fixed

- `Error::Conversion` says what was wrong and names the key: unknown, missing and duplicate fields
  by name, a wrong type by the key's path, what was expected and the kind of value found, as in
  ``invalid configuration at `server.port`: expected u16, found a string``.
- A typed read quotes no value: not serde's `invalid type: string "..."`, nor a `custom` message,
  nor an unknown enum variant.
- `#!yml` selects the format registered with `.yml`: a format answers to its name and to each
  extension fixed to it, wherever a format is named.
- A parse error's line counts a removed format line.

### Added

- Require an explicit default format in `Loader::new(&formats, default_format)`;
  registered file extensions and content markers can still take precedence.
- Automatic format selection registered per extension; `.cfg` is enabled by
  default and inherits the loader's default format when no header matches.
- `Formats::register_auto` to pin an extension-specific default and
  `Formats::register_extension` to configure fixed formats.
- `Loader::default_format` and `Options::default_format` for unnamed streams,
  byte slices, and automatically detected extensions without a pinned default.
- General-purpose `to_auto_vec`/`to_auto_vec_typed` writers for registered
  auto-detect extensions.
- `user_config_path`, `system_config_path` and `default_configs` using standard
  platform directories. `default_configs` yields system then user paths, with
  missing files represented by `None`.
- `Search`: the first file of an explicit path, a named variable's path, the user's and the
  system's, with `Ok(None)` when none exists and an error naming a path that was named and is not
  there. The working directory is never searched.
- `Formats::load` and `Formats::load_typed`, which read a file bounded by `Options::max_bytes`.
  `Error::File` names the path of whatever failed, and a `Loader`'s path source reports the same way.
- `Options::max_depth`: a typed read refuses deeper nesting (`Error::TooDeep`), default 64.
  `Loader::max_depth` holds for `Loader::load_typed` too.
- `CodecError::at_offset`, for a parser that reports a byte offset, and `CodecError` builder methods.
- `Detection::lines_removed`.
- Bounded parsing and reading from byte slices, strings, streams and paths.
- Optional source layers and `load_or` for an explicit missing-file fallback.
- MPL-2.0 licence, `CLA.md` and `CONTRIBUTING.md`.

[Unreleased]: https://github.com/jose-pr/anycfg-rs/commits/main/
