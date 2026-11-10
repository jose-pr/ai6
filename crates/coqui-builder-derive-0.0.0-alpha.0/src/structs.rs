// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A struct: named fields are a group, one field (a newtype, or a transparent struct) is that
//! field's setting.

use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt;
use syn::{Fields, Generics, Ident, Visibility};

use crate::attrs::{ContainerAttrs, FieldAttrs, RESERVED_SETTERS};
use crate::fields::FieldPlan;

/// What every expansion of one type shares.
pub(crate) struct Ctx<'a> {
    pub(crate) name: &'a Ident,
    pub(crate) generics: &'a Generics,
    pub(crate) container: &'a ContainerAttrs,
    pub(crate) vis: &'a Visibility,
}

impl Ctx<'_> {
    /// The generics with every type parameter bound by `Describe + Inspect`.
    pub(crate) fn bounded(&self, extra: &[TokenStream]) -> Generics {
        self.bounded_in(&quote!(__cb), extra)
    }

    /// The same, naming the crate as `root`: `__cb` inside the anonymous `const`, the crate's
    /// own path outside it.
    pub(crate) fn bounded_in(&self, root: &TokenStream, extra: &[TokenStream]) -> Generics {
        let mut generics = self.generics.clone();
        let params: Vec<Ident> = generics.type_params().map(|p| p.ident.clone()).collect();
        let clause = generics.make_where_clause();
        for param in params {
            clause
                .predicates
                .push(syn::parse_quote!(#param: #root::Describe + #root::Inspect));
        }
        for bound in extra {
            clause.predicates.push(syn::parse_quote!(#bound));
        }
        generics
    }

    /// Where `coqui_builder` is, outside the anonymous `const` that renames it `__cb`.
    pub(crate) fn krate(&self) -> TokenStream {
        self.container
            .krate
            .as_ref()
            .map_or_else(|| quote!(::coqui_builder), |path| quote!(#path))
    }

    /// `PhantomData` over every type parameter, or nothing when there are none.
    pub(crate) fn marker(&self) -> (TokenStream, TokenStream) {
        let params: Vec<&Ident> = self.generics.type_params().map(|p| &p.ident).collect();
        if params.is_empty() {
            return (TokenStream::new(), TokenStream::new());
        }
        (
            quote!(__marker: ::core::marker::PhantomData<fn() -> (#(#params,)*)>,),
            quote!(__marker: ::core::marker::PhantomData,),
        )
    }
}

/// What a struct expands to: the part inside the anonymous `const`, and the part beside the
/// struct.
pub(crate) type Expansion = (TokenStream, TokenStream);

pub(crate) fn expand(
    ctx: &Ctx<'_>,
    fields: &Fields,
    ast_attrs_doc: Option<&String>,
) -> syn::Result<Expansion> {
    let container = ctx.container;
    if container.deny_unknown_fields && !matches!(fields, Fields::Named(_)) {
        return Err(syn::Error::new_spanned(
            fields,
            "`#[derive(Describe)]` reads `deny_unknown_fields` only on a struct with named \
             fields: nothing else has keys",
        ));
    }
    match fields {
        Fields::Named(named) if container.transparent => {
            let mut kept = Vec::new();
            for field in &named.named {
                let attrs = FieldAttrs::read(field)?;
                if !attrs.skip {
                    kept.push((field, attrs));
                }
            }
            let [(field, attrs)] = &kept[..] else {
                return Err(syn::Error::new_spanned(
                    named,
                    "`#[derive(Describe)]` needs exactly one field that is not \
                     `#[map(skip)]` on a `#[map(transparent)]` struct: its setting is that \
                     field's.",
                ));
            };
            if attrs.rename.is_some()
                || attrs.default.is_some()
                || attrs.flatten
                || attrs.with.is_some()
                || attrs.as_text
                || attrs.none.is_some()
            {
                return Err(syn::Error::new_spanned(
                    &field.ident,
                    "`#[derive(Describe)]` takes no `#[map(..)]` but `skip` on a transparent \
                     struct's fields: the struct has no key of its own.",
                ));
            }
            let skipped: Vec<Ident> = named
                .named
                .iter()
                .filter(|f| f.ident != field.ident)
                .filter_map(|f| f.ident.clone())
                .collect();
            Ok((
                wrap(
                    ctx,
                    &field.ty,
                    &syn::Member::Named(field.ident.clone().expect("named")),
                    &skipped,
                    ast_attrs_doc,
                ),
                TokenStream::new(),
            ))
        }
        Fields::Named(named) => group(ctx, named, ast_attrs_doc),
        Fields::Unnamed(unnamed) => {
            for field in &unnamed.unnamed {
                if let Some(attr) = field
                    .attrs
                    .iter()
                    .find(|a| a.path().is_ident("map") || a.path().is_ident("schema"))
                {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "`#[derive(Describe)]` takes no `#[map(..)]` or `#[schema(..)]` on a \
                         tuple struct's field: it has no key to describe",
                    ));
                }
            }
            let all: Vec<&syn::Field> = unnamed.unnamed.iter().collect();
            let [only] = &all[..] else {
                return Err(syn::Error::new_spanned(
                    unnamed,
                    "`#[derive(Describe)]` refuses a tuple struct of two or more fields: it \
                     has no keys. Give the fields names, or hold one type.",
                ));
            };
            Ok((
                wrap(
                    ctx,
                    &only.ty,
                    &syn::Member::Unnamed(syn::Index::from(0)),
                    &[],
                    ast_attrs_doc,
                ),
                TokenStream::new(),
            ))
        }
        Fields::Unit => Err(syn::Error::new_spanned(
            ctx.name,
            "`#[derive(Describe)]` refuses a unit struct: it holds no setting to describe.",
        )),
    }
}

/// A struct that is its one field.
fn wrap(
    ctx: &Ctx<'_>,
    inner: &syn::Type,
    member: &syn::Member,
    skipped: &[Ident],
    doc: Option<&String>,
) -> TokenStream {
    let name = ctx.name;
    let generics = ctx.bounded(&[]);
    let (impl_g, ty_g, where_g) = generics.split_for_impl();
    let doc = doc.map(|d| quote!(__d.notes_mut().description.get_or_insert_with(|| __cb::__private::String::from(#d));));
    let construct = match member {
        syn::Member::Named(field) => {
            quote!(Self { #field: __inner #(, #skipped: ::core::default::Default::default())* })
        }
        syn::Member::Unnamed(_) => quote!(Self(__inner)),
    };
    quote! {
        #[automatically_derived]
        impl #impl_g __cb::fill::Wrap for #name #ty_g #where_g {
            type Inner = #inner;
            fn wrap(__inner: #inner) -> Self { #construct }
            fn inner(&self) -> &#inner { &self.#member }
            fn into_inner(self) -> #inner { self.#member }
        }

        #[automatically_derived]
        impl #impl_g __cb::Describe for #name #ty_g #where_g {
            type Fill = __cb::fill::Wrapped<Self, <#inner as __cb::Describe>::Fill>;
            const OPTIONAL: bool = <#inner as __cb::Describe>::OPTIONAL;
            const LEAF: bool = <#inner as __cb::Describe>::LEAF;
            const MIXED: bool = <#inner as __cb::Describe>::MIXED;
            fn description() -> __cb::Description {
                let mut __d = <#inner as __cb::Describe>::description();
                #doc
                __d
            }
        }

        #[automatically_derived]
        impl #impl_g __cb::Inspect for #name #ty_g #where_g {
            fn at(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Datum> {
                __cb::Inspect::at(&self.#member, __at)
            }

            fn entries(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Entries> {
                __cb::Inspect::entries(&self.#member, __at)
            }
        }
    }
}

/// A struct of named fields: a group.
fn group(ctx: &Ctx<'_>, named: &syn::FieldsNamed, doc: Option<&String>) -> syn::Result<Expansion> {
    let name = ctx.name;
    let container = ctx.container;
    let mut plans: Vec<FieldPlan> = Vec::new();
    for (index, field) in named.named.iter().enumerate() {
        plans.push(FieldPlan::read(field, index, container.rename_all, name)?);
    }
    reject_duplicate_keys(&plans)?;
    if container.deny_unknown_fields
        && let Some(flat) = plans.iter().find(|p| p.flatten())
    {
        return Err(syn::Error::new(
            flat.span(),
            "`#[derive(Describe)]` cannot read `deny_unknown_fields` beside a \
             `#[map(flatten)]` field: which keys the flattened member declares is its \
             type's, and not known here.",
        ));
    }
    for plan in &plans {
        if plan.skipped() {
            continue;
        }
        let setter = plan.attrs.setter_name(&plan.ident);
        if RESERVED_SETTERS.contains(&setter.as_str()) {
            return Err(syn::Error::new(
                plan.span(),
                format!(
                    "`{setter}` is a method every builder has, so a setting cannot take it \
                     for its setter. Rename the setter with `#[builder(setter = \"..\")]`: \
                     the key stays `{}`.",
                    plan.key
                ),
            ));
        }
    }

    reject_duplicate_setters(&plans)?;
    let live: Vec<&FieldPlan> = plans.iter().filter(|p| !p.skipped()).collect();
    let flats: Vec<&FieldPlan> = live.iter().copied().filter(|p| p.flatten()).collect();
    let keyed: Vec<&FieldPlan> = live.iter().copied().filter(|p| !p.flatten()).collect();
    let bounds: Vec<TokenStream> = flats
        .iter()
        .map(|p| p.flatten_bound(&quote!(__cb)))
        .collect();
    let generics = ctx.bounded(&bounds);
    let (impl_g, ty_g, where_g) = generics.split_for_impl();
    let (marker_decl, marker_init) = ctx.marker();

    let codecs: Vec<&TokenStream> = plans.iter().filter_map(|p| p.codec_def.as_ref()).collect();

    // ---- the Fill struct ----
    let slot_decls = live.iter().map(|p| {
        let ident = &p.ident;
        let slot = p.slot();
        quote!(#ident: #slot,)
    });
    let slot_news = live.iter().map(|p| {
        let ident = &p.ident;
        let new = p.new_slot();
        quote!(#ident: #new,)
    });
    let slot_defaults = plans
        .iter()
        .filter(|p| !p.skipped() && !p.flatten())
        .filter_map(|p| {
            let ident = &p.ident;
            let ty = &p.ty;
            let default = p.attrs.default.as_ref()?;
            let expr = match default {
                crate::attrs::DefaultKind::Trait => {
                    quote!(<#ty as ::core::default::Default>::default())
                }
                crate::attrs::DefaultKind::Expr(expr) => quote!(#expr),
            };
            Some(quote! {
                {
                    let __v: #ty = #expr;
                    <_ as __cb::Fill<#ty>>::set_default(&mut __s.#ident, __v);
                }
            })
        });

    // ---- set / set_items / clear ----
    let set_arms = keyed.iter().map(|p| {
        let ident = &p.ident;
        let key = &p.key;
        let ty = &p.ty;
        quote! {
            #key => <_ as __cb::Fill<#ty>>::set(&mut self.#ident, __rest, __text)
                .map_err(|e| e.under(__first.clone())),
        }
    });
    let set_flats = flats.iter().map(|p| {
        let ident = &p.ident;
        let ty = &p.ty;
        quote! {
            match <_ as __cb::Fill<#ty>>::set(&mut self.#ident, __at, __text) {
                ::core::result::Result::Err(e) if e.is_unknown_here() => {}
                other => return other,
            }
        }
    });
    let items_arms = keyed.iter().map(|p| {
        let ident = &p.ident;
        let key = &p.key;
        let ty = &p.ty;
        quote! {
            #key => <_ as __cb::Fill<#ty>>::set_items(&mut self.#ident, __rest, __items)
                .map_err(|e| e.under(__first.clone())),
        }
    });
    let items_flats = flats.iter().map(|p| {
        let ident = &p.ident;
        let ty = &p.ty;
        quote! {
            match <_ as __cb::Fill<#ty>>::set_items(&mut self.#ident, __at, __items) {
                ::core::result::Result::Err(e) if e.is_unknown_here() => {}
                other => return other,
            }
        }
    });
    let clear_arms = keyed.iter().map(|p| {
        let ident = &p.ident;
        let key = &p.key;
        let ty = &p.ty;
        quote! {
            #key => <_ as __cb::Fill<#ty>>::clear(&mut self.#ident, __rest)
                .map_err(|e| e.under(__first.clone())),
        }
    });
    let clear_flats = flats.iter().map(|p| {
        let ident = &p.ident;
        let ty = &p.ty;
        quote! {
            match <_ as __cb::Fill<#ty>>::clear(&mut self.#ident, __at) {
                ::core::result::Result::Err(e) if e.is_unknown_here() => {}
                other => return other,
            }
        }
    });

    let none_arms = keyed.iter().map(|p| {
        let ident = &p.ident;
        let key = &p.key;
        let ty = &p.ty;
        quote!(#key => <_ as __cb::Fill<#ty>>::holds_none(&self.#ident, __rest),)
    });
    let none_flats = flats.iter().map(|p| {
        let ident = &p.ident;
        let ty = &p.ty;
        quote! {
            if <_ as __cb::Fill<#ty>>::holds_none(&self.#ident, __at) {
                return true;
            }
        }
    });

    let any_arms = keyed.iter().map(|p| {
        let ident = &p.ident;
        let key = &p.key;
        let ty = &p.ty;
        quote! {
            #key => <_ as __cb::Fill<#ty>>::put_any(&mut self.#ident, __rest, __value)
                .map_err(|e| e.under(__first.clone())),
        }
    });
    let any_flats = flats.iter().map(|p| {
        let ident = &p.ident;
        let ty = &p.ty;
        quote! {
            match <_ as __cb::Fill<#ty>>::put_any(&mut self.#ident, __at, __value) {
                ::core::result::Result::Err(e) if e.is_unknown_here() => {}
                other => return other,
            }
        }
    });

    // ---- set_default / from_value / put ----
    let put_fields = live.iter().map(|p| {
        let ident = &p.ident;
        let ty = &p.ty;
        quote!(<_ as __cb::Fill<#ty>>::put(&mut self.#ident, #ident);)
    });
    let all_idents: Vec<&Ident> = live.iter().map(|p| &p.ident).collect();
    let set_default = live.iter().map(|p| {
        let ident = &p.ident;
        let ty = &p.ty;
        quote!(<_ as __cb::Fill<#ty>>::set_default(&mut self.#ident, #ident);)
    });
    let from_value = live.iter().map(|p| {
        let ident = &p.ident;
        let built = p.slot_from_value(quote!(#ident));
        quote!(#ident: #built,)
    });

    // ---- build ----
    let build_lets = live.iter().map(|p| {
        let ident = &p.ident;
        let ty = &p.ty;
        let local = quote::format_ident!("__b_{}", ident.unraw());
        if p.flatten() {
            quote!(let #local = <_ as __cb::Fill<#ty>>::build(self.#ident, __at, __missing);)
        } else {
            let key = &p.key;
            quote! {
                let #local = <_ as __cb::Fill<#ty>>::build(
                    self.#ident,
                    &__at.child(__cb::Segment::Key(__cb::__private::String::from(#key))),
                    __missing,
                );
            }
        }
    });
    let build_fields = plans.iter().map(|p| {
        let ident = &p.ident;
        if p.skipped() {
            quote!(#ident: ::core::default::Default::default(),)
        } else {
            let local = quote::format_ident!("__b_{}", ident.unraw());
            quote!(#ident: #local?,)
        }
    });

    // ---- inspect ----
    let root_own = keyed.iter().map(|p| {
        let ident = &p.ident;
        let key = &p.key;
        quote! {
            if __cb::Inspect::at(&self.#ident, &[]).is_some()
                || __cb::Inspect::entries(&self.#ident, &[]).is_some()
            {
                __keys.push(__cb::__private::String::from(#key));
            }
        }
    });
    let root_flats: Vec<TokenStream> = flats
        .iter()
        .map(|p| {
            let ident = &p.ident;
            quote! {
                if let ::core::option::Option::Some(__cb::Entries::Keys(__more)) =
                    __cb::Inspect::entries(&self.#ident, &[])
                {
                    __keys.extend(__more);
                }
            }
        })
        .collect();
    let fill_at_arms = keyed.iter().map(|p| {
        let ident = &p.ident;
        let key = &p.key;
        quote!(#key => __cb::Inspect::at(&self.#ident, __rest),)
    });
    let fill_entries_arms = keyed.iter().map(|p| {
        let ident = &p.ident;
        let key = &p.key;
        quote!(#key => __cb::Inspect::entries(&self.#ident, __rest),)
    });
    let fill_at_flats = flats
        .iter()
        .map(|p| {
            let ident = &p.ident;
            quote! {
                if let ::core::option::Option::Some(__v) = __cb::Inspect::at(&self.#ident, __at) {
                    return ::core::option::Option::Some(__v);
                }
            }
        })
        .collect::<Vec<_>>();
    let fill_entries_flats = flats.iter().map(|p| {
        let ident = &p.ident;
        quote! {
            if let ::core::option::Option::Some(__v) = __cb::Inspect::entries(&self.#ident, __at) {
                return ::core::option::Option::Some(__v);
            }
        }
    }).collect::<Vec<_>>();
    let root_own_built = keyed.iter().map(|p| {
        let ident = &p.ident;
        let key = &p.key;
        let access = p.inspect_built(quote!(&self.#ident));
        let entries = p.entries_built(quote!(&self.#ident));
        quote! {
            {
                let __rest: &[__cb::Segment] = &[];
                if (#access).is_some() || (#entries).is_some() {
                    __keys.push(__cb::__private::String::from(#key));
                }
            }
        }
    });
    let built_at_arms = keyed.iter().map(|p| {
        let ident = &p.ident;
        let key = &p.key;
        let access = p.inspect_built(quote!(&self.#ident));
        quote!(#key => #access,)
    });
    let built_entries_arms = keyed.iter().map(|p| {
        let ident = &p.ident;
        let key = &p.key;
        let access = p.entries_built(quote!(&self.#ident));
        quote!(#key => #access,)
    });

    // ---- description ----
    let field_pushes = live.iter().map(|p| {
        if p.flatten() {
            let ty = &p.ty;
            quote!(__fields.extend(<#ty as __cb::Describe>::description().into_group_fields());)
        } else {
            let expr = p.field_expr();
            quote!(__fields.push(#expr);)
        }
    });
    let duplicates = (!flats.is_empty()).then(|| {
        quote! {
            if ::core::cfg!(debug_assertions) {
                for (__at, __field) in __fields.iter().enumerate() {
                    if __fields[..__at].iter().any(|__o| __o.key == __field.key) {
                        ::core::panic!(
                            "`{}`: the key `{}` is declared twice, once by a flattened field",
                            ::core::stringify!(#name),
                            __field.key
                        );
                    }
                }
            }
        }
    });
    let deny = container.deny_unknown_fields.then(|| {
        quote! {
            if let __cb::describe::Kind::Group(__g) = __d.kind_mut() {
                __g.deny_unknown_fields = true;
            }
        }
    });
    let doc = doc.map(|d| quote!(__d.notes_mut().description = ::core::option::Option::Some(__cb::__private::String::from(#d));));

    let flat_unknown = quote!(::core::result::Result::Err(__cb::FillError::unknown_key(
        __first.clone()
    )));
    let value_destructure = if all_idents.is_empty() {
        quote!(let _ = __value;)
    } else {
        quote!(let #name { #(#all_idents,)* .. } = __value;)
    };

    let krate = ctx.krate();
    let outside_bounds: Vec<TokenStream> = flats.iter().map(|p| p.flatten_bound(&krate)).collect();
    let outside = ctx.bounded_in(&krate, &outside_bounds);
    let setters = crate::setters::write(ctx, &outside, &live, &krate);
    let setter_names = &setters.names;
    let (setters_outside, setters_inside) = (setters.outside, setters.inside);
    let inside = quote! {
        #(#codecs)*

        #setters_inside

        #[doc(hidden)]
        pub struct __Fill #impl_g #where_g {
            #(#slot_decls)*
            #marker_decl
        }

        impl #impl_g ::core::default::Default for __Fill #ty_g #where_g {
            fn default() -> Self {
                let mut __s = Self {
                    #(#slot_news)*
                    #marker_init
                };
                #(#slot_defaults)*
                __s
            }
        }

        impl #impl_g ::core::fmt::Debug for __Fill #ty_g #where_g {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(stringify!(#name)).finish_non_exhaustive()
            }
        }

        impl #impl_g __cb::Inspect for __Fill #ty_g #where_g {
            fn at(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Datum> {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    return ::core::option::Option::None;
                };
                match &*__first.name() {
                    #(#fill_at_arms)*
                    _ => {
                        #(#fill_at_flats)*
                        ::core::option::Option::None
                    }
                }
            }

            fn entries(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Entries> {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    let mut __keys: __cb::__private::Vec<__cb::__private::String> =
                        __cb::__private::Vec::new();
                    #(#root_own)*
                    #(#root_flats)*
                    return ::core::option::Option::Some(__cb::Entries::Keys(__keys));
                };
                match &*__first.name() {
                    #(#fill_entries_arms)*
                    _ => {
                        #(#fill_entries_flats)*
                        ::core::option::Option::None
                    }
                }
            }
        }

        impl #impl_g __cb::Fill<#name #ty_g> for __Fill #ty_g #where_g {
            fn set(
                &mut self,
                __at: &[__cb::Segment],
                __text: &str,
            ) -> ::core::result::Result<(), __cb::FillError> {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    return ::core::result::Result::Err(__cb::FillError::new(
                        __cb::fill::FillErrorKind::Unsupported("a group is set one key at a time"),
                    ));
                };
                match &*__first.name() {
                    #(#set_arms)*
                    _ => {
                        #(#set_flats)*
                        #flat_unknown
                    }
                }
            }

            fn set_items(
                &mut self,
                __at: &[__cb::Segment],
                __items: &[__cb::__private::String],
            ) -> ::core::result::Result<(), __cb::FillError> {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    return ::core::result::Result::Err(__cb::FillError::new(
                        __cb::fill::FillErrorKind::Unsupported("a group is not a list"),
                    ));
                };
                match &*__first.name() {
                    #(#items_arms)*
                    _ => {
                        #(#items_flats)*
                        #flat_unknown
                    }
                }
            }

            fn clear(&mut self, __at: &[__cb::Segment]) -> ::core::result::Result<(), __cb::FillError> {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    return ::core::result::Result::Err(__cb::FillError::new(
                        __cb::fill::FillErrorKind::Unsupported("a group is cleared one key at a time"),
                    ));
                };
                match &*__first.name() {
                    #(#clear_arms)*
                    _ => {
                        #(#clear_flats)*
                        #flat_unknown
                    }
                }
            }

            fn set_default(&mut self, __value: #name #ty_g) {
                #value_destructure
                #(#set_default)*
            }

            fn build(
                self,
                __at: &__cb::KeyPath,
                __missing: &mut __cb::__private::Vec<__cb::KeyPath>,
            ) -> ::core::option::Option<#name #ty_g> {
                #(#build_lets)*
                ::core::option::Option::Some(#name {
                    #(#build_fields)*
                })
            }

            fn put(&mut self, __value: #name #ty_g) {
                #value_destructure
                #(#put_fields)*
            }

            fn put_any(
                &mut self,
                __at: &[__cb::Segment],
                __value: &mut dyn ::core::any::Any,
            ) -> ::core::result::Result<(), __cb::FillError>
            where
                #name #ty_g: 'static,
            {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    return __cb::fill::put_whole::<#name #ty_g, Self>(self, __value);
                };
                match &*__first.name() {
                    #(#any_arms)*
                    _ => {
                        #(#any_flats)*
                        #flat_unknown
                    }
                }
            }

            fn holds_none(&self, __at: &[__cb::Segment]) -> bool {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    return false;
                };
                match &*__first.name() {
                    #(#none_arms)*
                    _ => {
                        #(#none_flats)*
                        false
                    }
                }
            }

            fn from_value(__value: #name #ty_g) -> Self {
                #value_destructure
                Self {
                    #(#from_value)*
                    #marker_init
                }
            }
        }

        #[automatically_derived]
        impl #impl_g __cb::Describe for #name #ty_g #where_g {
            type Fill = __Fill #ty_g;
            const __SETTERS: &'static [&'static str] = &[#(#setter_names),*];
            fn description() -> __cb::Description {
                let mut __fields: __cb::__private::Vec<__cb::describe::Field> =
                    __cb::__private::Vec::new();
                #(#field_pushes)*
                #duplicates
                let mut __d = __cb::Description::group(__fields);
                #deny
                #doc
                __d
            }
        }

        #[automatically_derived]
        impl #impl_g __cb::Fields for #name #ty_g #where_g {}

        #[automatically_derived]
        impl #impl_g __cb::Inspect for #name #ty_g #where_g {
            fn at(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Datum> {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    return ::core::option::Option::None;
                };
                match &*__first.name() {
                    #(#built_at_arms)*
                    _ => {
                        #(#fill_at_flats)*
                        ::core::option::Option::None
                    }
                }
            }

            fn entries(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Entries> {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    let mut __keys: __cb::__private::Vec<__cb::__private::String> =
                        __cb::__private::Vec::new();
                    #(#root_own_built)*
                    #(#root_flats)*
                    return ::core::option::Option::Some(__cb::Entries::Keys(__keys));
                };
                match &*__first.name() {
                    #(#built_entries_arms)*
                    _ => {
                        #(#fill_entries_flats)*
                        ::core::option::Option::None
                    }
                }
            }
        }
    };
    Ok((inside, setters_outside))
}

fn reject_duplicate_setters(plans: &[FieldPlan]) -> syn::Result<()> {
    for (i, plan) in plans.iter().enumerate() {
        if plan.skipped() {
            continue;
        }
        let setter = plan.attrs.setter_name(&plan.ident);
        if let Some(earlier) = plans[..i]
            .iter()
            .find(|o| !o.skipped() && o.attrs.setter_name(&o.ident) == setter)
        {
            return Err(syn::Error::new(
                plan.span(),
                format!(
                    "fields `{}` and `{}` both have the setter `{setter}`. Rename one with \
                     `#[builder(setter = \"..\")]`.",
                    earlier.ident, plan.ident
                ),
            ));
        }
    }
    Ok(())
}

fn reject_duplicate_keys(plans: &[FieldPlan]) -> syn::Result<()> {
    for (i, plan) in plans.iter().enumerate() {
        if plan.skipped() || plan.flatten() {
            continue;
        }
        if let Some(earlier) = plans[..i]
            .iter()
            .find(|o| !o.skipped() && !o.flatten() && o.key == plan.key)
        {
            return Err(syn::Error::new(
                plan.span(),
                format!(
                    "two fields map to the key \"{}\": `{}` and `{}`. The second would \
                     silently replace the first. Rename one with \
                     `#[map(rename = \"...\")]`.",
                    plan.key, earlier.ident, plan.ident
                ),
            ));
        }
    }
    Ok(())
}
