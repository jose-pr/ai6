// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `#[derive(Configured)]` on a built type: its one `#[configured]` field holds the settings.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, LitStr};

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    match try_expand(input) {
        Ok(tokens) => tokens,
        Err(error) => error.to_compile_error(),
    }
}

fn try_expand(tokens: TokenStream) -> syn::Result<TokenStream> {
    let ast: DeriveInput = syn::parse2(tokens)?;
    let mut krate: syn::Path = syn::parse_quote!(::coqui_builder);
    for attr in ast.attrs.iter().filter(|a| a.path().is_ident("configured")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("crate") {
                let value = meta.value()?;
                krate = if value.peek(LitStr) {
                    value.parse::<LitStr>()?.parse()?
                } else {
                    value.parse()?
                };
                Ok(())
            } else {
                Err(meta.error(
                    "unrecognised `#[configured(...)]` option on a type. \
                     `#[derive(Configured)]` knows `crate`: where `coqui_builder` is re-exported",
                ))
            }
        })?;
    }
    let Data::Struct(data) = &ast.data else {
        return Err(syn::Error::new_spanned(
            &ast.ident,
            "`#[derive(Configured)]` is for a struct that holds its settings in one field",
        ));
    };
    let marked: Vec<(syn::Member, &syn::Type)> = match &data.fields {
        Fields::Named(named) => named
            .named
            .iter()
            .filter(|f| f.attrs.iter().any(|a| a.path().is_ident("configured")))
            .map(|f| (syn::Member::Named(f.ident.clone().expect("named")), &f.ty))
            .collect(),
        Fields::Unnamed(unnamed) => unnamed
            .unnamed
            .iter()
            .enumerate()
            .filter(|(_, f)| f.attrs.iter().any(|a| a.path().is_ident("configured")))
            .map(|(at, f)| (syn::Member::Unnamed(syn::Index::from(at)), &f.ty))
            .collect(),
        Fields::Unit => Vec::new(),
    };
    let [(member, settings)] = &marked[..] else {
        return Err(syn::Error::new_spanned(
            &ast.ident,
            if marked.is_empty() {
                "`#[derive(Configured)]` needs `#[configured]` on the one field that holds the \
                 settings; this struct has none"
            } else {
                "`#[derive(Configured)]` needs `#[configured]` on exactly one field, the one \
                 that holds the settings; this struct has several"
            },
        ));
    };
    for field in match &data.fields {
        Fields::Named(n) => n.named.iter().collect::<Vec<_>>(),
        Fields::Unnamed(u) => u.unnamed.iter().collect(),
        Fields::Unit => Vec::new(),
    } {
        for attr in field
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("configured"))
        {
            if !matches!(attr.meta, syn::Meta::Path(_)) {
                return Err(syn::Error::new_spanned(
                    attr,
                    "`#[configured]` on a field takes no options",
                ));
            }
        }
    }

    let name = &ast.ident;
    let mut generics = ast.generics.clone();
    generics
        .make_where_clause()
        .predicates
        .push(syn::parse_quote!(#settings: #krate::Describe + #krate::Inspect));
    let (impl_g, ty_g, where_g) = generics.split_for_impl();
    Ok(quote! {
        #[automatically_derived]
        impl #impl_g #krate::builder::Configured for #name #ty_g #where_g {
            type Settings = #settings;

            fn configured(&self) -> &dyn #krate::Inspect {
                &self.#member
            }
        }

        #[automatically_derived]
        impl #impl_g #krate::Check<#name #ty_g> for #krate::Profile #where_g {
            fn check(&self, subject: &#name #ty_g, report: &mut #krate::Report) {
                self.judge(
                    &<#settings as #krate::Describe>::description(),
                    #krate::builder::Configured::configured(subject),
                    report,
                );
            }
        }

        #[automatically_derived]
        impl #impl_g #name #ty_g #where_g {
            /// Every setting that differs from its default.
            pub fn non_defaults(&self) -> #krate::__private::Vec<#krate::NonDefault> {
                #krate::builder::Configured::non_defaults(self)
            }
        }
    })
}
