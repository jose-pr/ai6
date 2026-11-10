// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Derives for `coqui-builder`: describe, fill and build any struct field by field. Reached
//! through that crate's `derive` feature.
//!
//! `#[derive(Describe)]` writes everything a struct needs from one declaration:
//! `Describe` (what each key is), the fill that a `Partial` holds, `Inspect` (reading a key
//! of a value), and `Fields` for a struct of named fields.
//!
//! | declaration | setting |
//! | --- | --- |
//! | struct with named fields | a group, one key per field |
//! | newtype struct, or `#[map(transparent)]` | the field's own setting |
//! | enum of unit variants | a choice of names (`Leaf`) |
//! | enum with `#[map(tag = "k")]` | a group whose fields depend on the arm the tag names |
//! | enum with `#[map(untagged)]` | text arms and at most one structured arm |
//!
//! `#[derive(Builder)]` writes the builder of a parts struct (one settings field, the rest
//! `#[builder(input)]`; `build`, `name`, `fallible`, `entry` and `crate` on the struct), and
//! `#[derive(Configured)]` the settings view of a built type (`#[configured]` on its one
//! settings field). A struct that derives `Describe` also gets a trait of typed setters,
//! `<Name>Setters`.
//!
//! The spellings are the ones `guatiao`'s derives read, so a type can move between the two;
//! the vectors in `tests/vectors/attributes.json` hold both to the same meaning.
//!
//! # Refused with a sentence
//!
//! Each refusal says what to do instead and never quotes the input.
//!
//! - A type: a union; a unit struct; a tuple struct of two or more fields, or with `#[map]` or
//!   `#[schema]` on a field; a lifetime or const parameter; a generic enum; an enum with no
//!   variant; a type that holds itself; `#[schema]` on a type; `deny_unknown_fields` on a
//!   type without named fields or beside a `flatten` field; a transparent struct without
//!   exactly one field that is not skipped, or with `#[map]` on it but `skip`.
//! - Keys: two fields (or two variants, or two fields of an arm) that map to the same key; a
//!   field stored under its enum's tag; `rename_all` with a rule that is not
//!   `lowercase`, `snake_case`, `kebab-case` or `SCREAMING_SNAKE_CASE`; `tag` on a struct, or
//!   with `untagged`; an enum without `tag` or `untagged` whose variants hold fields.
//! - Variants: more than one `#[map(other)]`, one that holds no `String`, one that is renamed,
//!   or one on an untagged enum or on a variant that is not a unit; `rename` on an untagged
//!   variant that holds a value; an untagged variant with named fields or not holding exactly
//!   one value; `flatten` or `#[map]` inside an arm.
//! - Untagged arms, when the enum is compiled: more than one arm that is not a single value, a
//!   structured arm that is itself an untagged enum holding one, an arm that is an `Option`,
//!   and an enum with no unit variant and no single-value arm.
//! - Field options: an unknown key of `#[map]`, `#[schema]` or `#[builder]`; `alias` and
//!   `skip_serializing_if`; `flatten` beside any other option, on an `Option`, or on a type
//!   without named fields; `skip` beside `rename` or `default`; `as_text` beside `with`; `with`
//!   on an `Option`; `default` given twice, or on an `Option` without `none`; `none` beside
//!   `with` or `as_text`, on a field that is not an `Option`, without `default = <expr>`, or
//!   empty; `multiple_of` that is not above zero; `stricter` other than `"higher"` or
//!   `"lower"`; an empty `setter`; a setter that is a name every builder has; `input`, which
//!   is for a parts struct.
//! - `#[builder(setters_on = ..)]`: no type, a value that is not a type, an enum, a tuple struct,
//!   a transparent struct, or a settings type with generic parameters.

#![forbid(unsafe_code)]

mod attrs;
mod builder;
mod configured;
mod enums;
mod fields;
mod rename;
mod setters;
mod structs;
mod untagged;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, GenericParam};

use crate::attrs::ContainerAttrs;
use crate::structs::Ctx;

/// Describes a settings type: see the crate documentation of `coqui-builder`.
#[proc_macro_derive(Describe, attributes(map, schema, builder, leaf))]
pub fn describe(input: TokenStream) -> TokenStream {
    expand(input.into()).into()
}

/// Writes a builder for a parts struct: one field holds the settings and every other field is
/// a `#[builder(input)]`. See the crate documentation of `coqui-builder`.
///
/// `#[builder(build = X)]` names the type the parts build, by `From<Parts> for X`; with
/// `fallible` by `TryFrom<Parts> for X`. The builder is `XBuilder`, made by `X::builder()`
/// or, with `entry = "new"`, by `XBuilder::new()`.
///
/// An input's setter takes `impl Into<T>` for its type `T`, except an input whose type is
/// exactly one of the parts struct's type parameters: that setter takes the parameter itself,
/// so setting it infers the parameter and needs no turbofish (an `Option<C>`, a `Vec<C>` or a
/// `C::Item` still takes `impl Into<..>`). An input written as a primitive scalar (`bool`, `char`,
/// an integer, `f32`, `f64`) takes it as itself too, so a literal infers. The builder
/// also has `parts()`, which gives what `build` converts, for a terminal written by hand.
#[proc_macro_derive(Builder, attributes(builder))]
pub fn builder(input: TokenStream) -> TokenStream {
    builder::expand(input.into()).into()
}

/// Makes a built type `Configured`: its one `#[configured]` field holds the settings.
///
/// Also writes `impl Check<Self> for Profile` and an inherent `non_defaults`.
#[proc_macro_derive(Configured, attributes(configured))]
pub fn configured(input: TokenStream) -> TokenStream {
    configured::expand(input.into()).into()
}

/// Makes a type a `Leaf`, one setting that is one text: see the crate documentation of
/// `coqui-builder`.
///
/// An enum of unit variants reads and writes its variant names (`rename`, `rename_all`);
/// any other type reads and writes its `FromStr` and `Display`. `#[leaf(name = "..")]` names
/// the type a datum shows, `#[leaf(ordered)]` takes its order from `PartialOrd`, and
/// `#[leaf(display)]` makes a unit-only enum use `Display` and `FromStr` as well.
#[proc_macro_derive(Leaf, attributes(leaf, map, schema))]
pub fn leaf(input: TokenStream) -> TokenStream {
    match try_leaf(input.into()) {
        Ok(tokens) => tokens,
        Err(error) => error.to_compile_error(),
    }
    .into()
}

fn try_leaf(input: TokenStream2) -> syn::Result<TokenStream2> {
    let ast: DeriveInput = syn::parse2(input)?;
    let mut name = ast.ident.to_string();
    let (mut ordered, mut display) = (false, false);
    for attr in ast.attrs.iter().filter(|a| a.path().is_ident("leaf")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                name = meta.value()?.parse::<syn::LitStr>()?.value();
            } else if meta.path.is_ident("ordered") {
                ordered = true;
            } else if meta.path.is_ident("display") {
                display = true;
            } else {
                return Err(meta.error(
                    "unrecognised `#[leaf(...)]` option. `#[derive(Leaf)]` knows `name = \"..\"`, \
                     `ordered` and `display`",
                ));
            }
            Ok(())
        })?;
    }
    let container = ContainerAttrs::read(&ast.attrs, matches!(ast.data, Data::Enum(_)))?;
    let ident = &ast.ident;
    let all_units = matches!(&ast.data, Data::Enum(data) if data.variants.iter().all(|v| {
        matches!(v.fields, syn::Fields::Unit)
            || v.attrs.iter().any(|a| a.path().is_ident("map"))
    }));
    let order = ordered.then(|| {
        quote! {
            const ORDERED: bool = true;
            fn order(
                a: &Self,
                b: &Self,
            ) -> ::core::option::Option<::core::cmp::Ordering> {
                ::core::cmp::PartialOrd::partial_cmp(a, b)
            }
        }
    });
    let body = match &ast.data {
        Data::Enum(data) if all_units && !display => enums::unit_enum(
            ident,
            &name,
            &container,
            data,
            container.doc.as_ref(),
            order.unwrap_or_default(),
        )?,
        _ => {
            let (impl_g, ty_g, where_g) = ast.generics.split_for_impl();
            quote! {
                #[automatically_derived]
                impl #impl_g __cb::Leaf for #ident #ty_g #where_g {
                    const TYPE: &'static str = #name;
                    #order
                    fn to_text(&self) -> __cb::__private::String {
                        __cb::__private::ToString::to_string(self)
                    }
                    fn from_text(
                        text: &str,
                    ) -> ::core::result::Result<Self, __cb::describe::LeafError> {
                        text.parse::<Self>().map_err(|_| __cb::describe::LeafError::Invalid {
                            type_name: #name,
                        })
                    }
                }
            }
        }
    };
    let krate = container
        .krate
        .as_ref()
        .map_or_else(|| quote!(::coqui_builder), |path| quote!(#path));
    Ok(quote! {
        #[allow(non_camel_case_types, unused_qualifications, unused_variables, dead_code, clippy::all)]
        const _: () = {
            use #krate as __cb;
            #body
        };
    })
}

/// The expansion, over [`proc_macro2::TokenStream`] so a `#[test]` can call it.
pub(crate) fn expand(input: TokenStream2) -> TokenStream2 {
    match try_expand(input) {
        Ok(tokens) => tokens,
        Err(error) => error.to_compile_error(),
    }
}

fn try_expand(input: TokenStream2) -> syn::Result<TokenStream2> {
    let ast: DeriveInput = syn::parse2(input)?;
    for param in &ast.generics.params {
        match param {
            GenericParam::Type(_) => {}
            GenericParam::Lifetime(l) => {
                return Err(syn::Error::new_spanned(
                    l,
                    "`#[derive(Describe)]` describes settings that own their data: a type \
                     with a lifetime parameter borrows from somewhere else.",
                ));
            }
            GenericParam::Const(c) => {
                return Err(syn::Error::new_spanned(
                    c,
                    "`#[derive(Describe)]` does not take a const generic parameter: a key \
                     cannot depend on one.",
                ));
            }
        }
    }
    let is_enum = matches!(ast.data, Data::Enum(_));
    let container = ContainerAttrs::read(&ast.attrs, is_enum)?;
    if let (false, Some(param)) = (container.setters_on.is_empty(), ast.generics.params.first()) {
        return Err(syn::Error::new_spanned(
            param,
            "`#[builder(setters_on = ..)]` is not written for a settings type with generic parameters: name the builder's setters by hand",
        ));
    }
    let ctx = Ctx {
        name: &ast.ident,
        generics: &ast.generics,
        container: &container,
        vis: &ast.vis,
    };
    let doc = container.doc.as_ref();
    let (body, outside) = match &ast.data {
        Data::Struct(data) => structs::expand(&ctx, &data.fields, doc)?,
        Data::Enum(data) => (enums::expand(&ctx, data, doc)?, TokenStream2::new()),
        Data::Union(data) => {
            return Err(syn::Error::new_spanned(
                data.union_token,
                "`#[derive(Describe)]` supports structs with named fields and enums. A union \
                 has no readable field: which arm is live is not knowable from the type.",
            ));
        }
    };
    if !container.setters_on.is_empty() && outside.is_empty() {
        return Err(syn::Error::new(
            ast.ident.span(),
            "`#[builder(setters_on = ..)]` belongs on a struct with named fields, which is the one that has setters: not an enum, a newtype or a `#[map(transparent)]` struct",
        ));
    }
    let krate = container
        .krate
        .as_ref()
        .map_or_else(|| quote!(::coqui_builder), |path| quote!(#path));
    Ok(quote! {
        #outside

        #[allow(
            non_camel_case_types,
            unused_qualifications,
            unused_mut,
            unused_variables,
            dead_code,
            clippy::all
        )]
        const _: () = {
            use #krate as __cb;
            #body
        };
    })
}

#[cfg(test)]
mod tests;
