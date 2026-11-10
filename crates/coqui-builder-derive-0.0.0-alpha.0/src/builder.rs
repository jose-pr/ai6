// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `#[derive(Builder)]` on a parts struct: one settings field and any number of inputs.
//!
//! The expansion is the builder's public type and its impls, written beside the parts struct
//! with the parts struct's visibility; nothing in it is anonymous, since the builder is API.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::{Data, DeriveInput, Fields, Ident, LitStr, Type};

use crate::attrs::{RESERVED_SETTERS, doc_comment, option_inner};
use crate::setters::method_ident;

/// Keys of `#[builder(..)]` on a parts struct.
pub(crate) const PARTS_TYPE_KEYS: &[&str] = &["build", "name", "fallible", "entry", "crate"];
/// Keys of `#[builder(..)]` on a field of a parts struct.
pub(crate) const PARTS_FIELD_KEYS: &[&str] = &["input", "setter"];

/// What `#[builder(..)]` on the parts struct says.
struct Options {
    build: syn::Path,
    name: Option<Ident>,
    fallible: bool,
    entry_new: bool,
    krate: syn::Path,
}

/// One field that is an input.
struct Input {
    ident: Ident,
    /// The text the input is reported under when it is missing.
    name: String,
    setter: String,
    ty: Type,
    /// The `J` of an `Option<J>` input, which is never missing.
    optional: Option<Type>,
    doc: Option<String>,
}

/// Whether `ty` is exactly one of the type parameters of `generics`.
fn is_type_parameter(ty: &Type, generics: &syn::Generics) -> bool {
    let ty = match ty {
        Type::Group(group) => &*group.elem,
        Type::Paren(paren) => &*paren.elem,
        other => other,
    };
    let Type::Path(path) = ty else {
        return false;
    };
    path.qself.is_none()
        && generics
            .type_params()
            .any(|param| path.path.is_ident(&param.ident))
}

fn list(keys: &[&str]) -> String {
    keys.iter()
        .map(|k| format!("`{k}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn read_options(ast: &DeriveInput) -> syn::Result<Options> {
    let mut build = None;
    let mut name = None;
    let mut fallible = false;
    let mut entry_new = false;
    let mut krate = None;
    for attr in ast.attrs.iter().filter(|a| a.path().is_ident("builder")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("build") {
                build = Some(meta.value()?.parse::<syn::Path>()?);
            } else if meta.path.is_ident("name") {
                let text: LitStr = meta.value()?.parse()?;
                name = Some(text.parse::<Ident>().map_err(|_| {
                    syn::Error::new_spanned(&text, "`name` is the builder's type name")
                })?);
            } else if meta.path.is_ident("fallible") {
                fallible = true;
            } else if meta.path.is_ident("entry") {
                let text: LitStr = meta.value()?.parse()?;
                match text.value().as_str() {
                    "new" => entry_new = true,
                    "builder" => entry_new = false,
                    _ => {
                        return Err(syn::Error::new_spanned(
                            text,
                            "`entry` is \"new\" (`XBuilder::new()`, for a type this crate does \
                             not own) or \"builder\" (`X::builder()`, the default)",
                        ));
                    }
                }
            } else if meta.path.is_ident("crate") {
                let value = meta.value()?;
                krate = Some(if value.peek(LitStr) {
                    value.parse::<LitStr>()?.parse()?
                } else {
                    value.parse()?
                });
            } else {
                return Err(meta.error(format!(
                    "unrecognised `#[builder(...)]` option on a parts struct. \
                     `#[derive(Builder)]` knows {} here",
                    list(PARTS_TYPE_KEYS)
                )));
            }
            Ok(())
        })?;
    }
    let build = build.ok_or_else(|| {
        syn::Error::new_spanned(
            &ast.ident,
            "`#[derive(Builder)]` needs `#[builder(build = X)]`: the type the parts build",
        )
    })?;
    Ok(Options {
        build,
        name,
        fallible,
        entry_new,
        krate: krate.unwrap_or_else(|| syn::parse_quote!(::coqui_builder)),
    })
}

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    match try_expand(input) {
        Ok(tokens) => tokens,
        Err(error) => error.to_compile_error(),
    }
}

fn try_expand(tokens: TokenStream) -> syn::Result<TokenStream> {
    let ast: DeriveInput = syn::parse2(tokens)?;
    let options = read_options(&ast)?;
    let named = match &ast.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(named) => named,
            _ => {
                return Err(syn::Error::new_spanned(
                    &ast.ident,
                    "`#[derive(Builder)]` needs a parts struct with named fields: one settings \
                     field and the inputs",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &ast.ident,
                "`#[derive(Builder)]` is for a parts struct: a struct with named fields",
            ));
        }
    };

    // The built type's name, and so the builder's.
    let built_last = options
        .build
        .segments
        .last()
        .ok_or_else(|| syn::Error::new_spanned(&options.build, "`build` names a type"))?;
    if options.name.is_none() && !matches!(built_last.arguments, syn::PathArguments::None) {
        return Err(syn::Error::new_spanned(
            &options.build,
            "`build = X` names the builder `XBuilder`, and a type with generic arguments has no \
             name to take it from. Name the builder with `name = \"..\"`; a type this crate \
             does not own also needs `entry = \"new\"`",
        ));
    }
    let builder = options
        .name
        .clone()
        .unwrap_or_else(|| format_ident!("{}Builder", built_last.ident));

    // Sort the fields into the settings and the inputs.
    let mut settings: Option<(&Ident, &Type)> = None;
    let mut inputs: Vec<Input> = Vec::new();
    for field in &named.named {
        let ident = field.ident.clone().expect("a named field");
        let mut is_input = false;
        let mut setter = None;
        for attr in field.attrs.iter().filter(|a| a.path().is_ident("builder")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("input") {
                    is_input = true;
                } else if meta.path.is_ident("setter") {
                    let text: LitStr = meta.value()?.parse()?;
                    if text.value().is_empty() {
                        return Err(syn::Error::new_spanned(
                            text,
                            "`setter` names a method, not nothing",
                        ));
                    }
                    crate::setters::check_method_name(&text.value(), text.span())?;
                    setter = Some(text.value());
                } else {
                    return Err(meta.error(format!(
                        "unrecognised `#[builder(...)]` option on a field of a parts struct. \
                         `#[derive(Builder)]` knows {} here",
                        list(PARTS_FIELD_KEYS)
                    )));
                }
                Ok(())
            })?;
        }
        if !is_input {
            if setter.is_some() {
                return Err(syn::Error::new_spanned(
                    &ident,
                    "`setter` renames the method of an input; the settings field has none, \
                     since each of its settings has a setter of its own",
                ));
            }
            if settings.is_some() {
                return Err(syn::Error::new_spanned(
                    &ident,
                    "a parts struct has one settings field, and this is a second. Mark a value \
                     that is not a setting `#[builder(input)]`, or hold the settings in one \
                     type",
                ));
            }
            settings = Some((field.ident.as_ref().expect("named"), &field.ty));
            continue;
        }
        let optional = option_inner(&field.ty).cloned();
        let name = ident.unraw().to_string();
        let setter = setter.unwrap_or_else(|| name.clone());
        if RESERVED_SETTERS.contains(&setter.as_str()) {
            return Err(syn::Error::new_spanned(
                &ident,
                format!(
                    "`{setter}` is a method every builder has, so an input cannot take it for \
                     its setter. Rename the setter with `#[builder(setter = \"..\")]`"
                ),
            ));
        }
        if let Some(earlier) = inputs.iter().find(|i| i.setter == setter) {
            return Err(syn::Error::new_spanned(
                &ident,
                format!(
                    "inputs `{}` and `{name}` both have the setter `{setter}`. Rename one with \
                     `#[builder(setter = \"..\")]`",
                    earlier.name
                ),
            ));
        }
        inputs.push(Input {
            ident,
            name,
            setter,
            ty: field.ty.clone(),
            optional,
            doc: doc_comment(&field.attrs),
        });
    }
    let Some((settings_field, settings_ty)) = settings else {
        return Err(syn::Error::new_spanned(
            &ast.ident,
            "a parts struct has one settings field, and this has none. The field that is not an \
             input holds the settings; mark every other field `#[builder(input)]`",
        ));
    };

    let krate = &options.krate;
    let vis = &ast.vis;
    let parts = &ast.ident;
    let built = &options.build;
    let has_generics = !ast.generics.params.is_empty();
    if has_generics && !options.entry_new {
        return Err(syn::Error::new_spanned(
            &ast.generics,
            "a parts struct with generic parameters needs `entry = \"new\"`: `X::builder()` \
             could not name them. The builder is made with `XBuilder::new()`",
        ));
    }

    let mut generics = ast.generics.clone();
    generics
        .make_where_clause()
        .predicates
        .push(syn::parse_quote!(#settings_ty: #krate::Describe));
    let (impl_g, ty_g, where_g) = generics.split_for_impl();
    let (_, parts_ty_g, _) = ast.generics.split_for_impl();

    let fields: Vec<Ident> = inputs
        .iter()
        .map(|i| format_ident!("__input_{}", i.ident.unraw()))
        .collect();
    let field_tys = inputs.iter().map(|i| match &i.optional {
        Some(_) => {
            let ty = &i.ty;
            quote!(#ty)
        }
        None => {
            let ty = &i.ty;
            quote!(::core::option::Option<#ty>)
        }
    });
    let nones = fields
        .iter()
        .map(|f| quote!(#f: ::core::option::Option::None,));

    // ---- the input setters ----
    let setter_fns = inputs.iter().zip(&fields).map(|(input, field)| {
        let name = method_ident(&input.setter, input.ident.span());
        let value_ty = input.optional.as_ref().unwrap_or(&input.ty);
        let doc = input
            .doc
            .clone()
            .unwrap_or_else(|| format!("Sets the input `{}`; the last call wins.", input.name));
        // A type parameter taken through `impl Into<C>` could never be inferred from the
        // argument, so an input that is exactly one takes it as it is; so does a primitive
        // scalar, so that a literal infers: for an `Option<u32>` input the `u32`.
        if (input.optional.is_none() && is_type_parameter(&input.ty, &ast.generics))
            || crate::setters::is_primitive_scalar(value_ty)
        {
            return quote! {
                #[doc = #doc]
                pub fn #name(mut self, value: #value_ty) -> Self {
                    self.#field = ::core::option::Option::Some(value);
                    self
                }
            };
        }
        quote! {
            #[doc = #doc]
            pub fn #name(mut self, value: impl ::core::convert::Into<#value_ty>) -> Self {
                self.#field = ::core::option::Option::Some(::core::convert::Into::into(value));
                self
            }
        }
    });

    // ---- build ----
    let required: Vec<(&Input, &Ident)> = inputs
        .iter()
        .zip(&fields)
        .filter(|(i, _)| i.optional.is_none())
        .collect();
    let required_checks = required.iter().map(|(i, f)| {
        let name = &i.name;
        quote! {
            if self.#f.is_none() {
                __inputs.push(#name);
            }
        }
    });
    let required_fields: Vec<&Ident> = required.iter().map(|(_, f)| *f).collect();
    let locals: Vec<Ident> = required
        .iter()
        .map(|(i, _)| format_ident!("__got_{}", i.ident.unraw()))
        .collect();
    let part_inits = inputs.iter().zip(&fields).map(|(input, field)| {
        let ident = &input.ident;
        match &input.optional {
            Some(_) => quote!(#ident: self.#field,),
            None => {
                let local = format_ident!("__got_{}", input.ident.unraw());
                quote!(#ident: #local,)
            }
        }
    });
    let (ret, missing, finish) = if options.fallible {
        (
            quote! {
                ::core::result::Result<
                    #built,
                    #krate::builder::BuildError<
                        <#built as ::core::convert::TryFrom<#parts #parts_ty_g>>::Error
                    >,
                >
            },
            quote!(#krate::builder::BuildError::Missing(__missing)),
            quote! {
                <#built as ::core::convert::TryFrom<#parts #parts_ty_g>>::try_from(__parts)
                    .map_err(#krate::builder::BuildError::Failed)
            },
        )
    } else {
        (
            quote!(::core::result::Result<#built, #krate::Missing>),
            quote!(__missing),
            quote! {
                ::core::result::Result::Ok(
                    <#built as ::core::convert::From<#parts #parts_ty_g>>::from(__parts)
                )
            },
        )
    };

    // ---- the entry ----
    let entry = if options.entry_new {
        quote! {
            impl #impl_g #builder #ty_g #where_g {
                /// A builder with nothing set.
                pub fn new() -> Self {
                    Self::__empty()
                }
            }

            #[automatically_derived]
            impl #impl_g ::core::default::Default for #builder #ty_g #where_g {
                fn default() -> Self {
                    Self::__empty()
                }
            }
        }
    } else {
        quote! {
            impl #built {
                /// A builder with nothing set.
                pub fn builder() -> #builder {
                    #builder::__empty()
                }
            }
        }
    };

    // ---- an input is never a setter of the settings ----
    let setter_names: Vec<&str> = inputs.iter().map(|i| i.setter.as_str()).collect();
    let assertions = setter_names.iter().map(|name| {
        let message = format!(
            "the input setter `{name}` is also a setter of the settings: rename it with \
             `#[builder(setter = \"..\")]`"
        );
        quote! {
            ::core::assert!(
                !#krate::__private::contains(
                    <#settings_ty as #krate::Describe>::__SETTERS,
                    #name
                ),
                #message
            );
        }
    });
    let assertions: Vec<TokenStream> = assertions.collect();
    let (free_assert, assoc_assert, use_assert) = if has_generics {
        (
            quote!(),
            quote! { const __INPUT_SETTERS: () = { #(#assertions)* }; },
            quote! { let () = Self::__INPUT_SETTERS; },
        )
    } else {
        (
            quote! { const _: () = { #(#assertions)* }; },
            quote!(),
            quote!(),
        )
    };

    // ---- the builder's Debug shows which inputs are set, never a value ----
    let debug_fields = inputs.iter().zip(&fields).map(|(input, field)| {
        let name = &input.name;
        quote!(.field(#name, &if self.#field.is_some() { "set" } else { "unset" }))
    });
    let builder_name = builder.to_string();
    let doc = format!(
        "Builds a `{}` from its settings, filled in layers, and the inputs it takes. \
         `build` lists everything still missing at once.",
        built_last.ident
    );

    Ok(quote! {
        #[doc = #doc]
        #[must_use = "a builder does nothing until it is built"]
        #vis struct #builder #impl_g #where_g {
            __partial: #krate::Partial<#settings_ty>,
            #(#fields: #field_tys,)*
        }

        #free_assert

        impl #impl_g #builder #ty_g #where_g {
            #assoc_assert

            fn __empty() -> Self {
                #use_assert
                Self {
                    __partial: #krate::Partial::new(),
                    #(#nones)*
                }
            }

            #(#setter_fns)*

            /// Lays `partial` over what is set: only the keys it sets are taken, and the last
            /// write of a key wins.
            pub fn fill(mut self, partial: #krate::Partial<#settings_ty>) -> Self {
                self.__partial.overlay(partial);
                self
            }

            /// Fills the settings from `source`, in turn after what is already set. The keys
            /// the source holds that the settings lack are dropped; to see them, fill a
            /// `Partial` and pass it to [`fill`](Self::fill).
            ///
            /// # Errors
            ///
            /// The `FillError` that `Partial::fill_from` gives.
            pub fn fill_from(
                mut self,
                source: &mut impl #krate::Source,
            ) -> ::core::result::Result<Self, #krate::FillError> {
                self.__partial.fill_from(source)?;
                ::core::result::Result::Ok(self)
            }

            /// The parts: the settings built, every required input present and every optional
            /// input as given. A second terminal (one that builds a different value from the
            /// same parts) is written over this.
            ///
            /// # Errors
            ///
            /// Everything that is missing, at once: the same `Missing`
            /// that `build` gives.
            pub fn parts(self) -> ::core::result::Result<#parts #parts_ty_g, #krate::Missing> {
                let mut __inputs: #krate::__private::Vec<&'static str> = #krate::__private::Vec::new();
                #(#required_checks)*
                let (__settings, __keys) = match self.__partial.build() {
                    ::core::result::Result::Ok(__s) => {
                        (::core::option::Option::Some(__s), #krate::__private::Vec::new())
                    }
                    ::core::result::Result::Err(__m) => {
                        (::core::option::Option::None, __m.keys().to_vec())
                    }
                };
                match (__settings, #(self.#required_fields,)*) {
                    (::core::option::Option::Some(__settings), #(::core::option::Option::Some(#locals),)*) => {
                        ::core::result::Result::Ok(#parts {
                            #settings_field: __settings,
                            #(#part_inits)*
                        })
                    }
                    _ => ::core::result::Result::Err(#krate::Missing::new(__keys, __inputs)),
                }
            }

            /// The built value, or everything that is missing: every required setting and
            /// every required input, at once.
            ///
            /// # Errors
            ///
            /// What is missing, or for a fallible build the error of the conversion.
            pub fn build(self) -> #ret {
                match self.parts() {
                    ::core::result::Result::Ok(__parts) => #finish,
                    ::core::result::Result::Err(__missing) => {
                        ::core::result::Result::Err(#missing)
                    }
                }
            }
        }

        #entry

        #[automatically_derived]
        impl #impl_g ::core::convert::From<#krate::Partial<#settings_ty>> for #builder #ty_g #where_g {
            fn from(partial: #krate::Partial<#settings_ty>) -> Self {
                let mut __builder = Self::__empty();
                __builder.__partial = partial;
                __builder
            }
        }

        #[automatically_derived]
        impl #impl_g #krate::builder::Configured for #builder #ty_g #where_g {
            type Settings = #settings_ty;

            fn configured(&self) -> &dyn #krate::Inspect {
                &self.__partial
            }
        }

        #[automatically_derived]
        impl #impl_g #krate::Check<#builder #ty_g> for #krate::Profile #where_g {
            fn check(&self, subject: &#builder #ty_g, report: &mut #krate::Report) {
                self.judge(
                    &<#settings_ty as #krate::Describe>::description(),
                    #krate::builder::Configured::configured(subject),
                    report,
                );
            }
        }

        #[automatically_derived]
        impl #impl_g #builder #ty_g #where_g {
            /// Every setting that differs from its default, as the builder would build it.
            pub fn non_defaults(&self) -> #krate::__private::Vec<#krate::NonDefault> {
                #krate::builder::Configured::non_defaults(self)
            }
        }

        #[automatically_derived]
        impl #impl_g #krate::builder::Holds<#settings_ty> for #builder #ty_g #where_g {
            fn partial_mut(&mut self) -> &mut #krate::Partial<#settings_ty> {
                &mut self.__partial
            }
        }

        #[automatically_derived]
        impl #impl_g ::core::fmt::Debug for #builder #ty_g #where_g {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(#builder_name)
                    #(#debug_fields)*
                    .finish_non_exhaustive()
            }
        }
    })
}
