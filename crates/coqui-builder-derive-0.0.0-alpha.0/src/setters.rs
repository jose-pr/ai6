// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The typed setters of a settings struct: a trait declared beside the struct, with one
//! method per setting, and a blanket impl for every builder that holds the struct's partial.
//!
//! The trait is outside the derive's anonymous `const`, so another crate can import it; the
//! impl is inside, since it names the hidden fill.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::{Attribute, Generics, Ident, Visibility};

use crate::fields::FieldPlan;
use crate::structs::Ctx;

/// Refuses a setter name that no method can have (`self`, `max-size`, `1x`).
pub(crate) fn check_method_name(name: &str, span: Span) -> syn::Result<()> {
    if syn::parse_str::<Ident>(name).is_ok()
        || syn::parse_str::<Ident>(&format!("r#{name}")).is_ok()
    {
        return Ok(());
    }
    Err(syn::Error::new(
        span,
        format!("`{name}` cannot be a setter: a setter is the name of a method"),
    ))
}

/// A method named `name`: a keyword is written raw.
pub(crate) fn method_ident(name: &str, span: Span) -> Ident {
    if syn::parse_str::<Ident>(name).is_ok() {
        Ident::new(name, span)
    } else {
        Ident::new_raw(name, span)
    }
}

/// The doc attributes of a field, or a sentence naming its key.
fn docs(attrs: &[Attribute], key: &str) -> Vec<TokenStream> {
    let found: Vec<TokenStream> = attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .map(|a| quote!(#a))
        .collect();
    if found.is_empty() {
        let text = format!("Sets `{key}`; the last call wins.");
        vec![quote!(#[doc = #text])]
    } else {
        found
    }
}

/// What a struct's setters add to its expansion.
pub(crate) struct Setters {
    /// The trait, which sits outside the anonymous `const`.
    pub(crate) outside: TokenStream,
    /// The blanket impl, which sits inside it.
    pub(crate) inside: TokenStream,
    /// The setters' names, for `Describe::__SETTERS`.
    pub(crate) names: Vec<String>,
}

/// Whether `ty` is written as a primitive scalar: a single-segment path with no generics, so a
/// literal infers against it. An alias of one is not recognised.
pub(crate) fn is_primitive_scalar(ty: &syn::Type) -> bool {
    const SCALARS: [&str; 16] = [
        "bool", "char", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
        "i128", "isize", "f32", "f64",
    ];
    let ty = match ty {
        syn::Type::Group(group) => &*group.elem,
        syn::Type::Paren(paren) => &*paren.elem,
        other => other,
    };
    let syn::Type::Path(path) = ty else {
        return false;
    };
    path.qself.is_none()
        && path.path.leading_colon.is_none()
        && path.path.segments.len() == 1
        && path.path.segments[0].arguments.is_empty()
        && SCALARS.iter().any(|name| path.path.is_ident(name))
}

/// The type a setter's argument is written with: a primitive scalar itself, any other type
/// as `impl Into<T>`.
pub(crate) fn argument(ty: &syn::Type) -> TokenStream {
    if is_primitive_scalar(ty) {
        quote!(#ty)
    } else {
        quote!(impl ::core::convert::Into<#ty>)
    }
}

pub(crate) fn write(
    ctx: &Ctx<'_>,
    generics: &Generics,
    live: &[&FieldPlan],
    krate: &TokenStream,
) -> Setters {
    let name = ctx.name;
    let vis: &Visibility = ctx.vis;
    let trait_name = format_ident!("{}Setters", name.unraw());
    let (_, ty_g, _) = generics.split_for_impl();
    let methods: Vec<(Ident, &FieldPlan)> = live
        .iter()
        .map(|p| {
            let setter = p.attrs.setter_name(&p.ident);
            (method_ident(&setter, p.ident.span()), *p)
        })
        .collect();
    let names: Vec<String> = live.iter().map(|p| p.attrs.setter_name(&p.ident)).collect();

    let declarations = methods.iter().map(|(method, plan)| {
        let ty = &plan.ty;
        let arg = argument(ty);
        let docs = docs(&plan.docs, &plan.key);
        quote! {
            #(#docs)*
            fn #method(self, value: #arg) -> Self;
        }
    });
    let definitions = methods.iter().map(|(method, plan)| {
        let (ty, ident) = (&plan.ty, &plan.ident);
        let arg = argument(ty);
        quote! {
            fn #method(mut self, value: #arg) -> Self {
                let __v: #ty = ::core::convert::Into::into(value);
                let __fill = __cb::Partial::__fill_mut(
                    <Self as __cb::builder::Holds<#name #ty_g>>::partial_mut(&mut self),
                );
                <_ as __cb::Fill<#ty>>::put(&mut __fill.#ident, __v);
                self
            }
        }
    });

    let where_g = generics.where_clause.as_ref();
    let (impl_params, _, _) = generics.split_for_impl();
    let doc = format!("The typed setters of `{name}`, for any builder that holds its settings.");

    let inherent = ctx.container.setters_on.iter().map(|target| {
        let (_, _, on_where) = target.generics.split_for_impl();
        let impl_generics = &target.generics;
        let ty = &target.ty;
        let mut predicates = on_where
            .map(|w| w.predicates.iter().map(|p| quote!(#p)).collect::<Vec<_>>())
            .unwrap_or_default();
        predicates.push(quote!(Self: #krate::builder::Holds<#name #ty_g>));
        let (on_impl, _, _) = impl_generics.split_for_impl();
        let methods = methods.iter().map(|(method, plan)| {
            let arg = argument(&plan.ty);
            let docs = docs(&plan.docs, &plan.key);
            quote! {
                #(#docs)*
                #vis fn #method(self, value: #arg) -> Self {
                    <Self as #trait_name #ty_g>::#method(self, value)
                }
            }
        });
        quote! {
            #[automatically_derived]
            #[allow(dead_code, clippy::should_implement_trait)]
            impl #on_impl #ty where #(#predicates),* {
                #(#methods)*
            }
        }
    });
    let outside = quote! {
        #[doc = #doc]
        #vis trait #trait_name #impl_params:
            #krate::builder::Holds<#name #ty_g> + ::core::marker::Sized #where_g
        {
            #(#declarations)*
        }

        #(#inherent)*
    };

    // The impl's own generics: the struct's, and the builder type `B` that holds the partial.
    let mut with_b = generics.clone();
    with_b.params.insert(0, syn::parse_quote!(__B));
    with_b
        .make_where_clause()
        .predicates
        .push(syn::parse_quote!(__B: #krate::builder::Holds<#name #ty_g>));
    let (impl_b, _, where_b) = with_b.split_for_impl();
    let inside = quote! {
        #[automatically_derived]
        impl #impl_b #trait_name #ty_g for __B #where_b {
            #(#definitions)*
        }
    };
    Setters {
        outside,
        inside,
        names,
    }
}
