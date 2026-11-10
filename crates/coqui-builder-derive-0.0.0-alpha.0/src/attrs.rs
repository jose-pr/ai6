// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Reading `#[map(..)]`, `#[schema(..)]` and `#[builder(..)]`.
//!
//! Every key each reader dispatches on is listed in a `const` below. The test that reads
//! `tests/vectors/attributes.json` fails when a key is in a list and in no case, or in a case
//! and in no list, so the spellings and their meaning stay the ones the vectors state.

use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Attribute, Expr, Field, Ident, LitStr, Type};

use crate::rename::RenameRule;

/// Keys of `#[map(..)]` on a field.
pub(crate) const MAP_FIELD_KEYS: &[&str] = &[
    "rename", "skip", "default", "flatten", "with", "as_text", "none",
];
/// Keys of `#[map(..)]` on a struct.
pub(crate) const MAP_STRUCT_KEYS: &[&str] =
    &["rename_all", "transparent", "deny_unknown_fields", "crate"];
/// Keys of `#[map(..)]` on an enum.
pub(crate) const MAP_ENUM_KEYS: &[&str] = &["tag", "untagged", "rename_all", "crate"];
/// Keys of `#[map(..)]` on a variant.
pub(crate) const MAP_VARIANT_KEYS: &[&str] = &["rename", "other"];
/// Keys of `#[schema(..)]` on a field.
pub(crate) const SCHEMA_FIELD_KEYS: &[&str] = &[
    "title",
    "description",
    "section",
    "order",
    "advanced",
    "sensitive",
    "default",
    "min_length",
    "max_length",
    "pattern",
    "format",
    "multiple_of",
    "min_items",
    "max_items",
    "read_only",
    "deprecated",
    "examples",
];
/// Keys of `#[schema(..)]` on a variant of a tagged enum.
pub(crate) const SCHEMA_ARM_KEYS: &[&str] = &["title", "description"];
/// Keys of `#[schema(..)]` on a variant of an enum of unit variants.
pub(crate) const SCHEMA_CHOICE_KEYS: &[&str] = &["label"];
/// Keys of `#[builder(..)]` on a field.
pub(crate) const BUILDER_FIELD_KEYS: &[&str] = &["stricter", "setter"];
/// Keys of `#[builder(..)]` on a type.
pub(crate) const BUILDER_TYPE_KEYS: &[&str] = &["crate", "setters_on"];

/// Method names every builder has: a setting cannot take one for its setter.
pub(crate) const RESERVED_SETTERS: &[&str] = &[
    "build",
    "parts",
    "fill",
    "fill_from",
    "builder",
    "new",
    "configured",
    "non_defaults",
    "partial_mut",
    "into",
    "try_into",
];

/// `a`, `b` and `c` as they are named in a message.
fn named(keys: &[&str]) -> String {
    keys.iter()
        .map(|k| format!("`{k}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// How a field's default is written.
pub(crate) enum DefaultKind {
    /// `#[map(default)]`: the type's own `Default`.
    Trait,
    /// `#[map(default = <expr>)]` or `#[schema(default = <expr>)]`.
    Expr(Expr),
}

/// What `#[schema(..)]` says about a field.
#[derive(Default)]
pub(crate) struct SchemaAttrs {
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) section: Option<String>,
    pub(crate) order: Option<i64>,
    pub(crate) advanced: bool,
    pub(crate) sensitive: bool,
    pub(crate) min_length: Option<u64>,
    pub(crate) max_length: Option<u64>,
    pub(crate) pattern: Option<String>,
    pub(crate) format: Option<String>,
    pub(crate) multiple_of: Option<f64>,
    pub(crate) min_items: Option<u64>,
    pub(crate) max_items: Option<u64>,
    pub(crate) read_only: bool,
    pub(crate) deprecated: bool,
    pub(crate) examples: Vec<Expr>,
    pub(crate) default: Option<Expr>,
}

/// What a field declares.
#[derive(Default)]
pub(crate) struct FieldAttrs {
    pub(crate) rename: Option<String>,
    pub(crate) skip: bool,
    pub(crate) default: Option<DefaultKind>,
    pub(crate) flatten: bool,
    pub(crate) with: Option<syn::Path>,
    pub(crate) as_text: bool,
    pub(crate) none: Option<String>,
    pub(crate) schema: SchemaAttrs,
    pub(crate) stricter: Option<bool>,
    pub(crate) setter: Option<String>,
}

/// A builder that gets the settings' setters as inherent methods: `setters_on = XBuilder` or
/// `setters_on = <'a, C: Channel> XBuilder<'a, C>`.
pub(crate) struct SettersOn {
    pub(crate) generics: syn::Generics,
    pub(crate) ty: syn::Type,
}

/// What a type declares.
#[derive(Default)]
pub(crate) struct ContainerAttrs {
    pub(crate) setters_on: Vec<SettersOn>,
    pub(crate) rename_all: Option<RenameRule>,
    pub(crate) transparent: bool,
    pub(crate) deny_unknown_fields: bool,
    pub(crate) tag: Option<String>,
    pub(crate) untagged: bool,
    pub(crate) krate: Option<syn::Path>,
    pub(crate) doc: Option<String>,
}

/// What a variant declares.
#[derive(Default)]
pub(crate) struct VariantAttrs {
    pub(crate) rename: Option<String>,
    pub(crate) other: Option<proc_macro2::Span>,
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) label: Option<String>,
}

/// A doc comment, as one line of text.
pub(crate) fn doc_comment(attrs: &[Attribute]) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        let syn::Meta::NameValue(nv) = &attr.meta else {
            continue;
        };
        let Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(text),
            ..
        }) = &nv.value
        else {
            continue;
        };
        let line = text.value().trim().to_string();
        if !line.is_empty() {
            lines.push(line);
        }
    }
    (!lines.is_empty()).then(|| lines.join(" "))
}

/// `Option<T>`'s `T`.
pub(crate) fn option_inner(ty: &Type) -> Option<&Type> {
    generic_arg(ty, "Option")
}

/// `Vec<T>`'s `T`.
pub(crate) fn vec_inner(ty: &Type) -> Option<&Type> {
    generic_arg(ty, "Vec")
}

fn generic_arg<'a>(ty: &'a Type, name: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != name {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    match args.args.first()? {
        syn::GenericArgument::Type(inner) if args.args.len() == 1 => Some(inner),
        _ => None,
    }
}

fn path_value(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<syn::Path> {
    let value = meta.value()?;
    if value.peek(LitStr) {
        value.parse::<LitStr>()?.parse()
    } else {
        value.parse()
    }
}

fn string(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<String> {
    Ok(meta.value()?.parse::<LitStr>()?.value())
}

fn int<T: core::str::FromStr>(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<T>
where
    T::Err: core::fmt::Display,
{
    meta.value()?.parse::<syn::LitInt>()?.base10_parse()
}

impl SchemaAttrs {
    fn read(&mut self, attr: &Attribute) -> syn::Result<()> {
        attr.parse_nested_meta(|meta| {
            let path = &meta.path;
            if path.is_ident("title") {
                self.title = Some(string(&meta)?);
            } else if path.is_ident("description") {
                self.description = Some(string(&meta)?);
            } else if path.is_ident("section") {
                self.section = Some(string(&meta)?);
            } else if path.is_ident("order") {
                self.order = Some(int(&meta)?);
            } else if path.is_ident("advanced") {
                self.advanced = true;
            } else if path.is_ident("sensitive") {
                self.sensitive = true;
            } else if path.is_ident("default") {
                self.default = Some(meta.value()?.parse()?);
            } else if path.is_ident("min_length") {
                self.min_length = Some(int(&meta)?);
            } else if path.is_ident("max_length") {
                self.max_length = Some(int(&meta)?);
            } else if path.is_ident("pattern") {
                self.pattern = Some(string(&meta)?);
            } else if path.is_ident("format") {
                self.format = Some(string(&meta)?);
            } else if path.is_ident("multiple_of") {
                let step = match meta.value()?.parse::<syn::Lit>()? {
                    syn::Lit::Int(i) => i.base10_parse::<f64>()?,
                    syn::Lit::Float(f) => f.base10_parse::<f64>()?,
                    other => {
                        return Err(syn::Error::new_spanned(other, "a number: `5` or `0.25`"));
                    }
                };
                if !(step > 0.0 && step.is_finite()) {
                    return Err(meta.error("`multiple_of` is a step greater than zero"));
                }
                self.multiple_of = Some(step);
            } else if path.is_ident("min_items") {
                self.min_items = Some(int(&meta)?);
            } else if path.is_ident("max_items") {
                self.max_items = Some(int(&meta)?);
            } else if path.is_ident("read_only") {
                self.read_only = true;
            } else if path.is_ident("deprecated") {
                self.deprecated = true;
            } else if path.is_ident("examples") {
                let content;
                syn::parenthesized!(content in meta.input);
                self.examples =
                    syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated(
                        &content,
                    )?
                    .into_iter()
                    .collect();
            } else {
                return Err(meta.error(format!(
                    "unrecognised `#[schema(...)]` option. `#[derive(Describe)]` knows {} on a \
                     field; the key and whether the field is required come from `#[map(...)]` \
                     and from the type.",
                    named(SCHEMA_FIELD_KEYS)
                )));
            }
            Ok(())
        })
    }
}

impl FieldAttrs {
    pub(crate) fn read(field: &Field) -> syn::Result<FieldAttrs> {
        let mut out = FieldAttrs::default();
        let mut default_span = None;
        out.schema.description = doc_comment(&field.attrs);

        for attr in &field.attrs {
            if attr.path().is_ident("schema") {
                out.schema.read(attr)?;
                continue;
            }
            if attr.path().is_ident("builder") {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("stricter") {
                        let text = string(&meta)?;
                        out.stricter = Some(match text.as_str() {
                            "higher" => true,
                            "lower" => false,
                            _ => {
                                return Err(meta.error(
                                    "`stricter` takes \"higher\" or \"lower\": which way the \
                                     setting gets stricter",
                                ));
                            }
                        });
                    } else if meta.path.is_ident("setter") {
                        out.setter = Some(string(&meta)?);
                    } else if meta.path.is_ident("input") {
                        return Err(meta.error(
                            "`input` belongs to a parts struct (`#[derive(Builder)]`): a settings type has settings only, and a value that is no text is an input of the parts struct that builds it",
                        ));
                    } else {
                        return Err(meta.error(format!(
                            "unrecognised `#[builder(...)]` option on a field. \
                             `#[derive(Describe)]` knows {} here",
                            named(BUILDER_FIELD_KEYS)
                        )));
                    }
                    Ok(())
                })?;
                continue;
            }
            if !attr.path().is_ident("map") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                let path = &meta.path;
                if path.is_ident("rename") {
                    out.rename = Some(string(&meta)?);
                } else if path.is_ident("skip") {
                    out.skip = true;
                } else if path.is_ident("flatten") {
                    out.flatten = true;
                } else if path.is_ident("with") {
                    out.with = Some(path_value(&meta)?);
                } else if path.is_ident("as_text") {
                    out.as_text = true;
                } else if path.is_ident("none") {
                    out.none = Some(string(&meta)?);
                } else if path.is_ident("default") {
                    if out.default.is_some() {
                        return Err(meta.error("`default` is given twice. Keep one."));
                    }
                    default_span = Some(meta.path.clone());
                    out.default = Some(if meta.input.peek(syn::Token![=]) {
                        DefaultKind::Expr(meta.value()?.parse()?)
                    } else {
                        DefaultKind::Trait
                    });
                } else if path.is_ident("skip_serializing_if") {
                    return Err(meta.error(
                        "`skip_serializing_if` has no meaning here: `#[derive(Describe)]` \
                         writes nothing, it only describes and fills. Remove it.",
                    ));
                } else {
                    return Err(meta.error(format!(
                        "unrecognised `#[map(...)]` option. `#[derive(Describe)]` knows {}. \
                         Not `alias`: a description refuses every key it does not declare, so \
                         an alias would name a key the description lacks.",
                        named(MAP_FIELD_KEYS)
                    )));
                }
                Ok(())
            })?;
        }

        let optional = option_inner(&field.ty).is_some();
        let named = field.ident.as_ref();
        let span_of = || -> proc_macro2::Span {
            named.map_or_else(proc_macro2::Span::call_site, Ident::span)
        };
        if out.flatten
            && (out.skip
                || out.rename.is_some()
                || default_span.is_some()
                || out.with.is_some()
                || out.as_text
                || out.none.is_some())
        {
            return Err(syn::Error::new(
                span_of(),
                "`flatten` stands alone: a flattened field has no key of its own to \
                 rename, skip, default or convert, since its type's keys are written \
                 beside this struct's.",
            ));
        }
        if out.flatten && optional {
            return Err(syn::Error::new_spanned(
                &field.ty,
                "`flatten` takes a type with named fields, and an `Option` may hold none, \
                 which has no keys to read beside the others.",
            ));
        }
        if out.as_text && out.with.is_some() {
            return Err(syn::Error::new(
                span_of(),
                "`as_text` and `with` both say how the field converts. Keep one: \
                 `as_text` for a type with `Display` and `FromStr`, `with` for any \
                 other shape.",
            ));
        }
        if out.with.is_some() && optional {
            return Err(syn::Error::new_spanned(
                &field.ty,
                "`with` converts a whole field, and an `Option` field may be left out, \
                 which the module cannot see. Convert an `Option<T>` in the module, on \
                 a field of that module's own type.",
            ));
        }
        if let Some(none) = &out.none {
            if out.with.is_some() || out.as_text {
                return Err(syn::Error::new(
                    span_of(),
                    "`none` cannot be combined with `with` or `as_text`: it names the text \
                     that writes the `None` of an `Option` of a type that reads no text.",
                ));
            }
            if !optional {
                return Err(syn::Error::new(
                    span_of(),
                    "`none` names the text that writes the `None` of an `Option` field: \
                     this field is not an `Option`.",
                ));
            }
            if !matches!(out.default, Some(DefaultKind::Expr(_))) && out.schema.default.is_none() {
                return Err(syn::Error::new(
                    span_of(),
                    "`none` needs `default = <expr>` beside it: the text is one more way to \
                     write a value, and the default says what an absent key means.",
                ));
            }
            if none.is_empty() {
                return Err(syn::Error::new(
                    span_of(),
                    "`none` names a text, not nothing",
                ));
            }
        }
        if out.skip
            && let Some(rename) = &out.rename
        {
            let _ = rename;
            return Err(syn::Error::new(
                span_of(),
                "`skip` and `rename` together say nothing: a field that is never \
                 stored has no key to rename. Remove one.",
            ));
        }
        if let Some(span) = default_span {
            if out.skip {
                return Err(syn::Error::new_spanned(
                    span,
                    "`skip` and `default` together say nothing: a field that is never \
                     stored always reads as its `Default`. Remove one.",
                ));
            }
            if optional && out.none.is_none() {
                return Err(syn::Error::new_spanned(
                    span,
                    "an `Option` field is already optional: an absent key reads as \
                     `None`. A default is for a plain `T`, or an `Option` whose `None` \
                     is written as text with `none = \"...\"`.",
                ));
            }
            if out.schema.default.is_some() {
                return Err(syn::Error::new_spanned(
                    span,
                    "`#[map(default)]` and `#[schema(default = ..)]` both name this \
                     field's default. `#[map(default = <expr>)]` says it once.",
                ));
            }
        } else if let Some(expr) = out.schema.default.take() {
            if optional && out.none.is_none() {
                return Err(syn::Error::new_spanned(
                    expr,
                    "an `Option` field is already optional: an absent key reads as `None`, \
                     so a default would never be written back. Make the field a plain `T` \
                     to give it a default.",
                ));
            }
            out.default = Some(DefaultKind::Expr(expr));
        }
        if let Some(setter) = &out.setter {
            if setter.is_empty() {
                return Err(syn::Error::new(
                    span_of(),
                    "`setter` names a method, not nothing",
                ));
            }
            crate::setters::check_method_name(setter, span_of())?;
        }
        Ok(out)
    }

    /// The method name this setting's setter would have.
    pub(crate) fn setter_name(&self, ident: &Ident) -> String {
        self.setter
            .clone()
            .unwrap_or_else(|| ident.unraw().to_string())
    }
}

impl ContainerAttrs {
    pub(crate) fn read(attrs: &[Attribute], is_enum: bool) -> syn::Result<ContainerAttrs> {
        let mut out = ContainerAttrs {
            doc: doc_comment(attrs),
            ..ContainerAttrs::default()
        };
        for attr in attrs {
            if attr.path().is_ident("builder") {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("crate") {
                        out.krate = Some(meta.value()?.parse()?);
                        Ok(())
                    } else if meta.path.is_ident("setters_on") {
                        let value = meta.value()?;
                        let generics: syn::Generics = value.parse()?;
                        let ty: syn::Type = value.parse().map_err(|e| {
                            syn::Error::new(
                                e.span(),
                                "`setters_on` takes the type of a builder in this crate: `setters_on = XBuilder`, or with generics `setters_on = <'a, C: Trait> XBuilder<'a, C>`",
                            )
                        })?;
                        out.setters_on.push(SettersOn { generics, ty });
                        Ok(())
                    } else {
                        Err(meta.error(format!(
                            "unrecognised `#[builder(...)]` option on a type. \
                             `#[derive(Describe)]` knows {} here: where `coqui_builder` is \
                             re-exported",
                            named(BUILDER_TYPE_KEYS)
                        )))
                    }
                })?;
                continue;
            }
            if attr.path().is_ident("schema") {
                return Err(syn::Error::new_spanned(
                    attr,
                    "`#[schema(..)]` describes a field or a variant: a type is described by \
                     its doc comment",
                ));
            }
            if !attr.path().is_ident("map") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                let path = &meta.path;
                if path.is_ident("crate") {
                    // guatiao's spelling: where guatiao is re-exported, which this derive
                    // does not use.
                    let _: syn::Path = meta.value()?.parse()?;
                    Ok(())
                } else if path.is_ident("rename_all") {
                    out.rename_all = Some(RenameRule::parse(&meta.value()?.parse::<LitStr>()?)?);
                    Ok(())
                } else if path.is_ident("transparent") && !is_enum {
                    out.transparent = true;
                    Ok(())
                } else if path.is_ident("deny_unknown_fields") && !is_enum {
                    out.deny_unknown_fields = true;
                    Ok(())
                } else if path.is_ident("untagged") && is_enum {
                    out.untagged = true;
                    Ok(())
                } else if path.is_ident("tag") && is_enum {
                    out.tag = Some(string(&meta)?);
                    Ok(())
                } else if path.is_ident("tag") {
                    Err(meta.error(
                        "`tag` names the key a VARIANT's name is stored under, so \
                         `#[derive(Describe)]` has nothing to do with one on a struct: a \
                         struct is one shape and has no variants to tell apart.",
                    ))
                } else if is_enum {
                    Err(meta.error(format!(
                        "unrecognised `#[map(...)]` option on an enum. `#[derive(Describe)]` \
                         knows {}: `tag` is the key a variant's name is stored under, \
                         `untagged` tells variants apart by the kind of value they hold.",
                        named(MAP_ENUM_KEYS)
                    )))
                } else {
                    Err(meta.error(format!(
                        "unrecognised `#[map(...)]` option on a struct. `#[derive(Describe)]` \
                         takes {} here: `rename`, `skip` and `default` go on a field, and \
                         `tag` is an enum's.",
                        named(MAP_STRUCT_KEYS)
                    )))
                }
            })?;
        }
        Ok(out)
    }
}

impl VariantAttrs {
    pub(crate) fn read(attrs: &[Attribute], tagged: bool) -> syn::Result<VariantAttrs> {
        let mut out = VariantAttrs::default();
        let doc = doc_comment(attrs);
        for attr in attrs {
            if attr.path().is_ident("map") {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("rename") {
                        out.rename = Some(string(&meta)?);
                        Ok(())
                    } else if meta.path.is_ident("other") {
                        out.other = Some(meta.path.span());
                        Ok(())
                    } else {
                        Err(meta.error(format!(
                            "unrecognised `#[map(...)]` option on a variant. \
                             `#[derive(Describe)]` knows {}: `other` is for the one variant \
                             of a unit enum that keeps a name it does not know. A variant \
                             cannot be skipped: a value of that variant would have no way to \
                             be described.",
                            named(MAP_VARIANT_KEYS)
                        )))
                    }
                })?;
            } else if attr.path().is_ident("schema") {
                attr.parse_nested_meta(|meta| {
                    if tagged && meta.path.is_ident("title") {
                        out.title = Some(string(&meta)?);
                        Ok(())
                    } else if tagged && meta.path.is_ident("description") {
                        out.description = Some(string(&meta)?);
                        Ok(())
                    } else if !tagged && meta.path.is_ident("label") {
                        out.label = Some(string(&meta)?);
                        Ok(())
                    } else if tagged {
                        Err(meta.error(format!(
                            "unrecognised `#[schema(...)]` option on a variant. \
                             `#[derive(Describe)]` knows {}: a variant is an arm. The other \
                             options describe fields, which go on its fields.",
                            named(SCHEMA_ARM_KEYS)
                        )))
                    } else {
                        Err(meta.error(format!(
                            "unrecognised `#[schema(...)]` option on a variant. \
                             `#[derive(Describe)]` knows {}: a unit variant is one choice \
                             among several. An arm of a tagged enum takes `title`.",
                            named(SCHEMA_CHOICE_KEYS)
                        )))
                    }
                })?;
            }
        }
        if tagged {
            if out.description.is_none() {
                out.description = doc;
            }
        } else if out.label.is_none() {
            out.label = doc;
        }
        Ok(out)
    }
}
