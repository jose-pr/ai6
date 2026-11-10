// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An enum: unit variants are a choice, `#[map(untagged)]` variants are told apart by the kind
//! of value, and `#[map(tag = "..")]` variants are arms of a group.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::{DataEnum, Fields, Ident, Type};

use crate::attrs::{ContainerAttrs, VariantAttrs};
use crate::fields::FieldPlan;
use crate::structs::Ctx;
use crate::untagged::{Held, Named, expand as untagged_union};

pub(crate) fn expand(
    ctx: &Ctx<'_>,
    data: &DataEnum,
    doc: Option<&String>,
) -> syn::Result<TokenStream> {
    let name = ctx.name;
    let container = ctx.container;
    if ctx.generics.type_params().next().is_some() {
        return Err(syn::Error::new_spanned(
            name,
            "`#[derive(Describe)]` supports a generic struct, not a generic enum: an enum's \
             arms are named by the derive, and a type parameter has no name to give one.",
        ));
    }
    if data.variants.is_empty() {
        return Err(syn::Error::new_spanned(
            name,
            "`#[derive(Describe)]` needs at least one variant. An enum with none has no value \
             to describe.",
        ));
    }
    if container.untagged {
        if container.tag.is_some() {
            return Err(syn::Error::new_spanned(
                name,
                "`#[derive(Describe)]` takes `tag` or `untagged`, not both: a tag stores each \
                 variant's name under a key, and an untagged enum stores no name.",
            ));
        }
        return untagged(name, container, data, doc);
    }
    if container.tag.is_none()
        && let Some(carrying) = data
            .variants
            .iter()
            .find(|v| !matches!(v.fields, Fields::Unit) && !says_other(v))
    {
        return Err(syn::Error::new_spanned(
            &carrying.ident,
            "`#[derive(Describe)]` needs `#[map(tag = \"...\")]` on an enum whose variants \
             carry fields. Two variants describe two different groups, and the key that tells \
             them apart (\"type\", \"kind\", a field name) is a decision this crate \
             deliberately does not make for you. Name it, and each arm's fields sit beside it.",
        ));
    }
    match &container.tag {
        None => unit_enum(name, &name.to_string(), container, data, doc, quote!()),
        Some(tag) => tagged(ctx, tag, data, doc),
    }
}

fn says_other(variant: &syn::Variant) -> bool {
    variant.attrs.iter().any(|attr| {
        let mut found = false;
        if attr.path().is_ident("map") {
            let _ = attr.parse_nested_meta(|meta| {
                found |= meta.path.is_ident("other");
                if meta.input.peek(syn::Token![=]) {
                    meta.value()?.parse::<syn::Expr>()?;
                }
                Ok(())
            });
        }
        found
    })
}

/// A variant's stored name.
fn stored(variant: &syn::Variant, attrs: &VariantAttrs, container: &ContainerAttrs) -> String {
    attrs.rename.clone().unwrap_or_else(|| {
        let name = variant.ident.unraw().to_string();
        container
            .rename_all
            .map_or(name.clone(), |r| r.variant(&name))
    })
}

fn reject_duplicates(names: &[(String, &Ident)]) -> syn::Result<()> {
    for (i, (name, ident)) in names.iter().enumerate() {
        if let Some((_, earlier)) = names[..i].iter().find(|(n, _)| n == name) {
            return Err(syn::Error::new(
                ident.span(),
                format!(
                    "two variants are stored as \"{name}\": `{earlier}` and `{ident}`. Rename \
                     one with `#[map(rename = \"...\")]`."
                ),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// An enum of unit variants.

pub(crate) fn unit_enum(
    name: &Ident,
    ty_name: &str,
    container: &ContainerAttrs,
    data: &DataEnum,
    doc: Option<&String>,
    order: TokenStream,
) -> syn::Result<TokenStream> {
    struct Unit {
        ident: Ident,
        name: String,
        label: Option<String>,
    }
    let mut units: Vec<Unit> = Vec::new();
    let mut other: Option<Ident> = None;
    for variant in &data.variants {
        let attrs = VariantAttrs::read(&variant.attrs, false)?;
        if let Some(span) = attrs.other {
            if other.is_some() {
                return Err(syn::Error::new(
                    variant.ident.span(),
                    "two variants are `#[map(other)]`. A name no variant declares could be \
                     read as either, so only one can be the catch-all.",
                ));
            }
            if attrs.rename.is_some() || attrs.label.is_some() {
                return Err(syn::Error::new(
                    span,
                    "the `other` variant has no name of its own: it is stored as whatever \
                     text it holds, so there is nothing to rename or to label. Remove \
                     `rename` and `label`.",
                ));
            }
            match &variant.fields {
                Fields::Unnamed(u) if u.unnamed.len() == 1 => {}
                _ => {
                    return Err(syn::Error::new_spanned(
                        &variant.ident,
                        "`#[derive(Describe)]` needs the `#[map(other)]` variant to hold the \
                         text it did not recognise: exactly one unnamed field, as \
                         `Other(String)`.",
                    ));
                }
            }
            other = Some(variant.ident.clone());
            continue;
        }
        units.push(Unit {
            ident: variant.ident.clone(),
            name: stored(variant, &attrs, container),
            label: attrs.label,
        });
    }
    let pairs: Vec<(String, &Ident)> = units.iter().map(|u| (u.name.clone(), &u.ident)).collect();
    reject_duplicates(&pairs)?;

    let ty_name = ty_name.to_owned();
    let listed: Vec<&String> = units.iter().map(|u| &u.name).collect();
    let to = units.iter().map(|u| {
        let (i, n) = (&u.ident, &u.name);
        quote!(Self::#i => __cb::__private::String::from(#n),)
    });
    let to_other = other
        .as_ref()
        .map(|o| quote!(Self::#o(__v) => ::core::clone::Clone::clone(__v),));
    let from = units.iter().map(|u| {
        let (i, n) = (&u.ident, &u.name);
        quote!(#n => ::core::result::Result::Ok(Self::#i),)
    });
    let fallback = match &other {
        Some(o) => {
            quote!(_ => ::core::result::Result::Ok(Self::#o(__cb::__private::String::from(__text))),)
        }
        None => quote!(_ => ::core::result::Result::Err(__cb::describe::LeafError::UnknownName),),
    };
    let _ = doc;
    let labelled: Vec<(&String, &String)> = units
        .iter()
        .filter_map(|u| u.label.as_ref().map(|l| (&u.name, l)))
        .collect();
    let labels = labels_fn(&labelled);
    Ok(quote! {
        #[automatically_derived]
        impl __cb::Leaf for #name {
            const TYPE: &'static str = #ty_name;
            #labels
            #order

            fn to_text(&self) -> __cb::__private::String {
                match self {
                    #(#to)*
                    #to_other
                }
            }

            fn from_text(__text: &str) -> ::core::result::Result<Self, __cb::describe::LeafError> {
                match __text {
                    #(#from)*
                    #fallback
                }
            }

            fn choices() -> ::core::option::Option<&'static [&'static str]> {
                ::core::option::Option::Some(&[#(#listed),*])
            }
        }
    })
}

/// `Leaf::labels` for the texts that have a label; nothing when none has.
fn labels_fn(labelled: &[(&String, &String)]) -> TokenStream {
    if labelled.is_empty() {
        return quote!();
    }
    let rows = labelled.iter().map(|(text, label)| quote!((#text, #label)));
    quote! {
        fn labels() -> &'static [(&'static str, &'static str)] {
            &[#(#rows),*]
        }
    }
}

// ---------------------------------------------------------------------------------------
// An untagged enum.

fn untagged(
    name: &Ident,
    container: &ContainerAttrs,
    data: &DataEnum,
    doc: Option<&String>,
) -> syn::Result<TokenStream> {
    let mut names: Vec<Named> = Vec::new();
    let mut held: Vec<Held> = Vec::new();
    for variant in &data.variants {
        if says_other(variant) {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "`other` has no place in an untagged enum: a string naming no unit variant is \
                 refused rather than kept, since another variant could be the one a reader in \
                 another language would choose.",
            ));
        }
        match &variant.fields {
            Fields::Unit => {
                let attrs = VariantAttrs::read(&variant.attrs, false)?;
                names.push((
                    variant.ident.clone(),
                    stored(variant, &attrs, container),
                    attrs.label,
                ));
            }
            Fields::Unnamed(u) if u.unnamed.len() == 1 => {
                let attrs = VariantAttrs::read(&variant.attrs, true)?;
                if attrs.rename.is_some() {
                    return Err(syn::Error::new_spanned(
                        &variant.ident,
                        "`rename` names what a variant is stored as, and an untagged variant \
                         holding a value stores no name.",
                    ));
                }
                crate::fields::refuse_self_reference(&u.unnamed[0].ty, name)?;
                held.push(Held {
                    ident: variant.ident.clone(),
                    ty: u.unnamed[0].ty.clone(),
                });
            }
            Fields::Unnamed(u) => {
                return Err(syn::Error::new_spanned(
                    u,
                    "`#[derive(Describe)]` needs a variant of an untagged enum to hold one \
                     value: `Name(T)`.",
                ));
            }
            Fields::Named(n) => {
                return Err(syn::Error::new_spanned(
                    n,
                    "`#[derive(Describe)]` cannot describe an untagged variant with named \
                     fields: keys need a tag to say which arm they belong to. Give the enum \
                     a `tag`, or hold one value: `Name(T)`.",
                ));
            }
        }
    }
    let pairs: Vec<(String, &Ident)> = names.iter().map(|(i, n, _)| (n.clone(), i)).collect();
    reject_duplicates(&pairs)?;

    if !held.is_empty() {
        let labelled: Vec<(&String, &String)> = names
            .iter()
            .filter_map(|(_, n, l)| l.as_ref().map(|l| (n, l)))
            .collect();
        let _ = (container, doc);
        return Ok(untagged_union(name, &names, &held, &labels_fn(&labelled)));
    }

    let to_names = names
        .iter()
        .map(|(i, n, _)| quote!(Self::#i => __cb::__private::String::from(#n),));
    let name_matches = names
        .iter()
        .map(|(i, n, _)| quote!(#n => return ::core::result::Result::Ok(Self::#i),));
    let text_names = if names.is_empty() {
        quote!()
    } else {
        quote!(match __text { #(#name_matches)* _ => {} })
    };
    let _ = container;
    let _ = doc;
    let labelled: Vec<(&String, &String)> = names
        .iter()
        .filter_map(|(_, n, l)| l.as_ref().map(|l| (n, l)))
        .collect();
    let labels = labels_fn(&labelled);
    Ok(quote! {
        #[automatically_derived]
        impl __cb::Leaf for #name {
            const TYPE: &'static str = stringify!(#name);
            #labels

            fn to_text(&self) -> __cb::__private::String {
                match self {
                    #(#to_names)*
                }
            }

            fn from_text(__text: &str) -> ::core::result::Result<Self, __cb::describe::LeafError> {
                #text_names
                ::core::result::Result::Err(__cb::describe::LeafError::Invalid {
                    type_name: stringify!(#name),
                })
            }
        }
    })
}

// ---------------------------------------------------------------------------------------
// A tagged enum.

enum ArmKind {
    Unit,
    Named(Vec<FieldPlan>),
    Newtype(Type),
}

struct Arm {
    ident: Ident,
    name: String,
    title: Option<String>,
    description: Option<String>,
    kind: ArmKind,
}

fn tagged(
    ctx: &Ctx<'_>,
    tag: &str,
    data: &DataEnum,
    doc: Option<&String>,
) -> syn::Result<TokenStream> {
    let name = ctx.name;
    let container = ctx.container;
    let mut arms: Vec<Arm> = Vec::new();
    for (index, variant) in data.variants.iter().enumerate() {
        let attrs = VariantAttrs::read(&variant.attrs, true)?;
        if let Some(span) = attrs.other {
            return Err(syn::Error::new(
                span,
                "`other` is for an enum of unit variants, where a name nobody declared is \
                 still a name. An unknown variant of a tagged enum would carry fields nobody \
                 declared, and nothing here could keep them.",
            ));
        }
        let kind = match &variant.fields {
            Fields::Unit => ArmKind::Unit,
            Fields::Unnamed(u) if u.unnamed.len() == 1 => {
                if let Some(attr) = u.unnamed[0]
                    .attrs
                    .iter()
                    .find(|a| a.path().is_ident("map") || a.path().is_ident("schema"))
                {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "`#[derive(Describe)]` takes no `#[map(..)]` or `#[schema(..)]` on a \
                         newtype variant's field: its keys are its type's, beside the tag.",
                    ));
                }
                ArmKind::Newtype(u.unnamed[0].ty.clone())
            }
            Fields::Unnamed(u) => {
                return Err(syn::Error::new_spanned(
                    u,
                    "`#[derive(Describe)]` needs a variant's fields to have names: they sit \
                     beside the tag under their own keys, and a tuple variant has no names to \
                     key them by. Give the fields names, or hold one type whose keys sit \
                     beside the tag: `Name(T)`.",
                ));
            }
            Fields::Named(named) => {
                let mut plans = Vec::new();
                for (i, field) in named.named.iter().enumerate() {
                    plans.push(FieldPlan::read(field, index * 1000 + i, None, name)?);
                }
                for (i, plan) in plans.iter().enumerate() {
                    if plan.skipped() || plan.flatten() {
                        if plan.flatten() {
                            return Err(syn::Error::new(
                                plan.span(),
                                "`#[derive(Describe)]` does not flatten inside an arm: hold \
                                 the type as the arm's one field, `Name(T)`.",
                            ));
                        }
                        continue;
                    }
                    if plan.key == tag {
                        return Err(syn::Error::new(
                            plan.span(),
                            format!(
                                "a field of `{}` is stored under \"{tag}\", the key the tag \
                                 uses. Rename the field with `#[map(rename = \"...\")]`.",
                                variant.ident
                            ),
                        ));
                    }
                    if let Some(earlier) = plans[..i]
                        .iter()
                        .find(|o| !o.skipped() && o.key == plan.key)
                    {
                        return Err(syn::Error::new(
                            plan.span(),
                            format!(
                                "two fields map to the key \"{}\": `{}` and `{}`. Rename one \
                                 with `#[map(rename = \"...\")]`.",
                                plan.key, earlier.ident, plan.ident
                            ),
                        ));
                    }
                }
                ArmKind::Named(plans)
            }
        };
        arms.push(Arm {
            ident: variant.ident.clone(),
            name: stored(variant, &attrs, container),
            title: attrs.title,
            description: attrs.description,
            kind,
        });
    }
    let pairs: Vec<(String, &Ident)> = arms.iter().map(|a| (a.name.clone(), &a.ident)).collect();
    reject_duplicates(&pairs)?;

    let bounds: Vec<TokenStream> = Vec::new();
    let generics = ctx.bounded(&bounds);
    let (_, _, where_g) = generics.split_for_impl();

    // Names of the slots.
    let slot_name = |arm: usize, field: &Ident| format_ident!("__a{}_{}", arm, field.unraw());
    let newtype_slot = |arm: usize| format_ident!("__a{}", arm);

    let codecs: Vec<&TokenStream> = arms
        .iter()
        .filter_map(|a| match &a.kind {
            ArmKind::Named(plans) => Some(plans.iter().filter_map(|p| p.codec_def.as_ref())),
            _ => None,
        })
        .flatten()
        .collect();

    let mut slot_decls = Vec::new();
    let mut slot_news = Vec::new();
    let mut slot_defaults = Vec::new();
    for (i, arm) in arms.iter().enumerate() {
        match &arm.kind {
            ArmKind::Unit => {}
            ArmKind::Named(plans) => {
                for p in plans.iter().filter(|p| !p.skipped()) {
                    let slot = slot_name(i, &p.ident);
                    let ty = p.slot();
                    slot_decls.push(quote!(#slot: #ty,));
                    let new = p.new_slot();
                    slot_news.push(quote!(#slot: #new,));
                    if let Some(default) = &p.attrs.default {
                        let fty = &p.ty;
                        let expr = match default {
                            crate::attrs::DefaultKind::Trait => {
                                quote!(<#fty as ::core::default::Default>::default())
                            }
                            crate::attrs::DefaultKind::Expr(e) => quote!(#e),
                        };
                        slot_defaults.push(quote! {
                            {
                                let __v: #fty = #expr;
                                <_ as __cb::Fill<#fty>>::set_default(&mut __s.#slot, __v);
                            }
                        });
                    }
                }
            }
            ArmKind::Newtype(ty) => {
                let slot = newtype_slot(i);
                slot_decls.push(quote!(#slot: <#ty as __cb::Describe>::Fill,));
                slot_news.push(quote!(#slot: ::core::default::Default::default(),));
            }
        }
    }

    // ---- descriptions ----
    let arm_exprs = arms.iter().map(|arm| {
        let n = &arm.name;
        let fields = match &arm.kind {
            ArmKind::Unit => quote!(__cb::__private::Vec::new()),
            ArmKind::Newtype(ty) => quote!(<#ty as __cb::Describe>::description().into_group_fields()),
            ArmKind::Named(plans) => {
                let pushes = plans.iter().filter(|p| !p.skipped()).map(|p| {
                    let e = p.field_expr();
                    quote!(__fields.push(#e);)
                });
                quote! {
                    {
                        let mut __fields: __cb::__private::Vec<__cb::describe::Field> =
                            __cb::__private::Vec::new();
                        #(#pushes)*
                        __fields
                    }
                }
            }
        };
        let title = arm.title.as_ref().map(|t| quote!(__arm.title = ::core::option::Option::Some(__cb::__private::String::from(#t));));
        let description = arm.description.as_ref().map(|t| quote!(__arm.description = ::core::option::Option::Some(__cb::__private::String::from(#t));));
        quote! {
            {
                let mut __arm = __cb::describe::Arm::new(#n, #fields);
                #title
                #description
                __arm
            }
        }
    });
    let doc = doc.map(|d| quote!(__d.notes_mut().description = ::core::option::Option::Some(__cb::__private::String::from(#d));));
    let newtype_bounds = arms.iter().filter_map(|a| match &a.kind {
        ArmKind::Newtype(ty) => Some(quote!(#ty: __cb::Fields)),
        _ => None,
    });
    let mut generics_b = ctx.bounded(&[]);
    {
        let clause = generics_b.make_where_clause();
        for b in newtype_bounds {
            clause.predicates.push(syn::parse_quote!(#b));
        }
    }
    let (_, _, where_b) = generics_b.split_for_impl();
    let _ = where_g;

    // ---- select ----
    let select_arms = arms.iter().enumerate().map(|(i, a)| {
        let n = &a.name;
        quote!(#n => { self.__arm = ::core::option::Option::Some(#i); ::core::result::Result::Ok(()) })
    });
    let name_arms = arms.iter().enumerate().map(|(i, a)| {
        let n = &a.name;
        quote!(#i => #n,)
    });

    // ---- set / set_text / clear over the arms' fields ----
    let mut set_fields = Vec::new();
    let mut text_fields = Vec::new();
    let mut clear_fields = Vec::new();
    for (i, arm) in arms.iter().enumerate() {
        match &arm.kind {
            ArmKind::Unit => {}
            ArmKind::Named(plans) => {
                for p in plans.iter().filter(|p| !p.skipped()) {
                    let slot = slot_name(i, &p.ident);
                    let (key, ty) = (&p.key, &p.ty);
                    set_fields.push(quote! {
                        if *__name == *#key {
                            match <_ as __cb::Fill<#ty>>::set(&mut self.#slot, __rest, __text) {
                                ::core::result::Result::Ok(()) => __accepted = true,
                                ::core::result::Result::Err(e) => {
                                    __failure.get_or_insert(e.under(__first.clone()));
                                }
                            }
                        }
                    });
                    text_fields.push(quote! {
                        if *__name == *#key {
                            match <_ as __cb::Fill<#ty>>::set_items(&mut self.#slot, __rest, __items) {
                                ::core::result::Result::Ok(()) => __accepted = true,
                                ::core::result::Result::Err(e) => {
                                    __failure.get_or_insert(e.under(__first.clone()));
                                }
                            }
                        }
                    });
                    clear_fields.push(quote! {
                        if *__name == *#key {
                            if <_ as __cb::Fill<#ty>>::clear(&mut self.#slot, __rest).is_ok() {
                                __accepted = true;
                            }
                        }
                    });
                }
            }
            ArmKind::Newtype(ty) => {
                let slot = newtype_slot(i);
                set_fields.push(quote! {
                    match <_ as __cb::Fill<#ty>>::set(&mut self.#slot, __at, __text) {
                        ::core::result::Result::Ok(()) => __accepted = true,
                        ::core::result::Result::Err(e) if e.is_unknown_here() => {}
                        ::core::result::Result::Err(e) => {
                            __failure.get_or_insert(e);
                        }
                    }
                });
                text_fields.push(quote! {
                    match <_ as __cb::Fill<#ty>>::set_items(&mut self.#slot, __at, __items) {
                        ::core::result::Result::Ok(()) => __accepted = true,
                        ::core::result::Result::Err(e) if e.is_unknown_here() => {}
                        ::core::result::Result::Err(e) => {
                            __failure.get_or_insert(e);
                        }
                    }
                });
                clear_fields.push(quote! {
                    if <_ as __cb::Fill<#ty>>::clear(&mut self.#slot, __at).is_ok() {
                        __accepted = true;
                    }
                });
            }
        }
    }

    // ---- set_default / from_value ----
    let mut default_arms = Vec::new();
    let mut value_arms = Vec::new();
    for (i, arm) in arms.iter().enumerate() {
        let v = &arm.ident;
        match &arm.kind {
            ArmKind::Unit => {
                default_arms.push(
                    quote!(#name::#v => { self.__default = ::core::option::Option::Some(#i); }),
                );
                value_arms
                    .push(quote!(#name::#v => { __s.__arm = ::core::option::Option::Some(#i); }));
            }
            ArmKind::Newtype(ty) => {
                let slot = newtype_slot(i);
                default_arms.push(quote! {
                    #name::#v(__v) => {
                        self.__default = ::core::option::Option::Some(#i);
                        <_ as __cb::Fill<#ty>>::set_default(&mut self.#slot, __v);
                    }
                });
                value_arms.push(quote! {
                    #name::#v(__v) => {
                        __s.__arm = ::core::option::Option::Some(#i);
                        __s.#slot = <<#ty as __cb::Describe>::Fill as __cb::Fill<#ty>>::from_value(__v);
                    }
                });
            }
            ArmKind::Named(plans) => {
                let idents: Vec<&Ident> = plans
                    .iter()
                    .filter(|p| !p.skipped())
                    .map(|p| &p.ident)
                    .collect();
                let sets = plans.iter().filter(|p| !p.skipped()).map(|p| {
                    let slot = slot_name(i, &p.ident);
                    let (id, ty) = (&p.ident, &p.ty);
                    quote!(<_ as __cb::Fill<#ty>>::set_default(&mut self.#slot, #id);)
                });
                let froms = plans.iter().filter(|p| !p.skipped()).map(|p| {
                    let slot = slot_name(i, &p.ident);
                    let id = &p.ident;
                    let built = p.slot_from_value(quote!(#id));
                    quote!(__s.#slot = #built;)
                });
                default_arms.push(quote! {
                    #name::#v { #(#idents,)* .. } => {
                        self.__default = ::core::option::Option::Some(#i);
                        #(#sets)*
                    }
                });
                value_arms.push(quote! {
                    #name::#v { #(#idents,)* .. } => {
                        __s.__arm = ::core::option::Option::Some(#i);
                        #(#froms)*
                    }
                });
            }
        }
    }

    // ---- put ----
    let mut put_arms = Vec::new();
    for (i, arm) in arms.iter().enumerate() {
        let v = &arm.ident;
        match &arm.kind {
            ArmKind::Unit => put_arms.push(quote! {
                #name::#v => { self.__arm = ::core::option::Option::Some(#i); }
            }),
            ArmKind::Newtype(ty) => {
                let slot = newtype_slot(i);
                put_arms.push(quote! {
                    #name::#v(__v) => {
                        self.__arm = ::core::option::Option::Some(#i);
                        <_ as __cb::Fill<#ty>>::put(&mut self.#slot, __v);
                    }
                });
            }
            ArmKind::Named(plans) => {
                let idents: Vec<&Ident> = plans
                    .iter()
                    .filter(|p| !p.skipped())
                    .map(|p| &p.ident)
                    .collect();
                let puts = plans.iter().filter(|p| !p.skipped()).map(|p| {
                    let slot = slot_name(i, &p.ident);
                    let (id, ty) = (&p.ident, &p.ty);
                    quote!(<_ as __cb::Fill<#ty>>::put(&mut self.#slot, #id);)
                });
                put_arms.push(quote! {
                    #name::#v { #(#idents,)* .. } => {
                        self.__arm = ::core::option::Option::Some(#i);
                        #(#puts)*
                    }
                });
            }
        }
    }

    // ---- put_any over the arms' fields ----
    let mut any_fields = Vec::new();
    for (i, arm) in arms.iter().enumerate() {
        match &arm.kind {
            ArmKind::Unit => {}
            ArmKind::Named(plans) => {
                for p in plans.iter().filter(|p| !p.skipped()) {
                    let slot = slot_name(i, &p.ident);
                    let (key, ty) = (&p.key, &p.ty);
                    any_fields.push(quote! {
                        if *__name == *#key && (__pass == 1 || __arm == ::core::option::Option::Some(#i)) {
                            match <_ as __cb::Fill<#ty>>::put_any(&mut self.#slot, __rest, __value) {
                                ::core::result::Result::Ok(()) => __accepted = true,
                                ::core::result::Result::Err(e) => {
                                    __failure.get_or_insert(e.under(__first.clone()));
                                }
                            }
                        }
                    });
                }
            }
            ArmKind::Newtype(ty) => {
                let slot = newtype_slot(i);
                any_fields.push(quote! {
                    if !__accepted && (__pass == 1 || __arm == ::core::option::Option::Some(#i)) {
                        match <_ as __cb::Fill<#ty>>::put_any(&mut self.#slot, __at, __value) {
                            ::core::result::Result::Ok(()) => __accepted = true,
                            ::core::result::Result::Err(e) if e.is_unknown_here() => {}
                            ::core::result::Result::Err(e) => {
                                __failure.get_or_insert(e);
                            }
                        }
                    }
                });
            }
        }
    }

    // ---- holds_none ----
    let mut none_fields = Vec::new();
    for (i, arm) in arms.iter().enumerate() {
        match &arm.kind {
            ArmKind::Unit => {}
            ArmKind::Named(plans) => {
                for p in plans.iter().filter(|p| !p.skipped()) {
                    let slot = slot_name(i, &p.ident);
                    let (key, ty) = (&p.key, &p.ty);
                    none_fields.push(quote! {
                        if (__arm.is_none() || __arm == ::core::option::Option::Some(#i))
                            && *__name == *#key
                        {
                            return <_ as __cb::Fill<#ty>>::holds_none(&self.#slot, __rest);
                        }
                    });
                }
            }
            ArmKind::Newtype(ty) => {
                let slot = newtype_slot(i);
                none_fields.push(quote! {
                    if (__arm.is_none() || __arm == ::core::option::Option::Some(#i))
                        && <_ as __cb::Fill<#ty>>::holds_none(&self.#slot, __at)
                    {
                        return true;
                    }
                });
            }
        }
    }

    // ---- build ----
    let build_arms = arms.iter().enumerate().map(|(i, arm)| {
        let v = &arm.ident;
        match &arm.kind {
            ArmKind::Unit => quote!(#i => ::core::option::Option::Some(#name::#v),),
            ArmKind::Newtype(ty) => {
                let slot = newtype_slot(i);
                quote! {
                    #i => {
                        let __b = <_ as __cb::Fill<#ty>>::build(self.#slot, __at, __missing)?;
                        ::core::option::Option::Some(#name::#v(__b))
                    }
                }
            }
            ArmKind::Named(plans) => {
                let lets = plans.iter().filter(|p| !p.skipped()).map(|p| {
                    let slot = slot_name(i, &p.ident);
                    let (id, ty, key) = (&p.ident, &p.ty, &p.key);
                    let local = format_ident!("__b_{}", id.unraw());
                    quote! {
                        let #local = <_ as __cb::Fill<#ty>>::build(
                            self.#slot,
                            &__at.child(__cb::Segment::Key(__cb::__private::String::from(#key))),
                            __missing,
                        );
                    }
                });
                let sets = plans.iter().map(|p| {
                    let id = &p.ident;
                    if p.skipped() {
                        quote!(#id: ::core::default::Default::default(),)
                    } else {
                        let local = format_ident!("__b_{}", id.unraw());
                        quote!(#id: #local?,)
                    }
                });
                quote! {
                    #i => {
                        #(#lets)*
                        ::core::option::Option::Some(#name::#v { #(#sets)* })
                    }
                }
            }
        }
    });

    // ---- inspect (fill) ----
    let mut fill_at_fields = Vec::new();
    let mut fill_entries_fields = Vec::new();
    let mut fill_keys = Vec::new();
    let here = |i: usize| quote!((__arm.is_none() || __arm == ::core::option::Option::Some(#i)));
    for (i, arm) in arms.iter().enumerate() {
        let h = here(i);
        match &arm.kind {
            ArmKind::Unit => {}
            ArmKind::Newtype(_) => {
                let slot = newtype_slot(i);
                fill_at_fields.push(quote! {
                    if #h {
                        if let ::core::option::Option::Some(__v) = __cb::Inspect::at(&self.#slot, __at) {
                            return ::core::option::Option::Some(__v);
                        }
                    }
                });
                fill_entries_fields.push(quote! {
                    if #h {
                        if let ::core::option::Option::Some(__v) = __cb::Inspect::entries(&self.#slot, __at) {
                            return ::core::option::Option::Some(__v);
                        }
                    }
                });
                fill_keys.push(quote! {
                    if #h {
                        if let ::core::option::Option::Some(__cb::Entries::Keys(__more)) =
                            __cb::Inspect::entries(&self.#slot, &[])
                        {
                            __keys.extend(__more);
                        }
                    }
                });
            }
            ArmKind::Named(plans) => {
                for p in plans.iter().filter(|p| !p.skipped()) {
                    let slot = slot_name(i, &p.ident);
                    let key = &p.key;
                    fill_at_fields.push(quote! {
                        if #h && *__name == *#key {
                            return __cb::Inspect::at(&self.#slot, __rest);
                        }
                    });
                    fill_entries_fields.push(quote! {
                        if #h && *__name == *#key {
                            return __cb::Inspect::entries(&self.#slot, __rest);
                        }
                    });
                    fill_keys.push(quote! {
                        if #h
                            && (__cb::Inspect::at(&self.#slot, &[]).is_some()
                                || __cb::Inspect::entries(&self.#slot, &[]).is_some())
                        {
                            __keys.push(__cb::__private::String::from(#key));
                        }
                    });
                }
            }
        }
    }

    // ---- inspect (built) ----
    let built_keys = arms.iter().map(|arm| {
        let v = &arm.ident;
        match &arm.kind {
            ArmKind::Unit => quote!(#name::#v => {}),
            ArmKind::Newtype(_) => quote! {
                #name::#v(__v) => {
                    if let ::core::option::Option::Some(__cb::Entries::Keys(__more)) =
                        __cb::Inspect::entries(__v, &[])
                    {
                        __keys.extend(__more);
                    }
                }
            },
            ArmKind::Named(plans) => {
                let idents: Vec<&Ident> = plans
                    .iter()
                    .filter(|p| !p.skipped())
                    .map(|p| &p.ident)
                    .collect();
                let pushes = plans.iter().filter(|p| !p.skipped()).map(|p| {
                    let id = &p.ident;
                    let key = &p.key;
                    let access = p.inspect_built(quote!(#id));
                    let entries = p.entries_built(quote!(#id));
                    quote! {
                        {
                            let __rest: &[__cb::Segment] = &[];
                            if (#access).is_some() || (#entries).is_some() {
                                __keys.push(__cb::__private::String::from(#key));
                            }
                        }
                    }
                });
                quote!(#name::#v { #(#idents,)* .. } => { #(#pushes)* })
            }
        }
    });
    let built_name = arms.iter().map(|arm| {
        let v = &arm.ident;
        let n = &arm.name;
        match &arm.kind {
            ArmKind::Unit => quote!(#name::#v => #n,),
            ArmKind::Newtype(_) => quote!(#name::#v(..) => #n,),
            ArmKind::Named(_) => quote!(#name::#v { .. } => #n,),
        }
    });
    let built_at = arms.iter().map(|arm| {
        let v = &arm.ident;
        match &arm.kind {
            ArmKind::Unit => quote!(#name::#v => ::core::option::Option::None,),
            ArmKind::Newtype(_) => quote!(#name::#v(__v) => __cb::Inspect::at(__v, __at),),
            ArmKind::Named(plans) => {
                let idents: Vec<&Ident> = plans
                    .iter()
                    .filter(|p| !p.skipped())
                    .map(|p| &p.ident)
                    .collect();
                let keyed = plans.iter().filter(|p| !p.skipped()).map(|p| {
                    let id = &p.ident;
                    let key = &p.key;
                    let access = p.inspect_built(quote!(#id));
                    quote!(#key => #access,)
                });
                quote! {
                    #name::#v { #(#idents,)* .. } => match &*__name {
                        #(#keyed)*
                        _ => ::core::option::Option::None,
                    },
                }
            }
        }
    });

    let built_entries = arms.iter().map(|arm| {
        let v = &arm.ident;
        match &arm.kind {
            ArmKind::Unit => quote!(#name::#v => ::core::option::Option::None,),
            ArmKind::Newtype(_) => quote!(#name::#v(__v) => __cb::Inspect::entries(__v, __at),),
            ArmKind::Named(plans) => {
                let idents: Vec<&Ident> = plans
                    .iter()
                    .filter(|p| !p.skipped())
                    .map(|p| &p.ident)
                    .collect();
                let keyed = plans.iter().filter(|p| !p.skipped()).map(|p| {
                    let id = &p.ident;
                    let key = &p.key;
                    let access = p.entries_built(quote!(#id));
                    quote!(#key => #access,)
                });
                quote! {
                    #name::#v { #(#idents,)* .. } => match &*__name {
                        #(#keyed)*
                        _ => ::core::option::Option::None,
                    },
                }
            }
        }
    });

    let tag_text = tag;
    Ok(quote! {
        #(#codecs)*

        #[doc(hidden)]
        pub struct __Fill {
            __arm: ::core::option::Option<usize>,
            __default: ::core::option::Option<usize>,
            #(#slot_decls)*
        }

        impl __Fill {
            fn __select(&mut self, __name: &str) -> ::core::result::Result<(), __cb::FillError> {
                match __name {
                    #(#select_arms)*
                    _ => ::core::result::Result::Err(__cb::FillError::leaf(
                        __cb::describe::LeafError::UnknownName,
                    )),
                }
            }

            fn __arm_name(__arm: usize) -> &'static str {
                match __arm {
                    #(#name_arms)*
                    _ => "",
                }
            }
        }

        impl ::core::default::Default for __Fill {
            fn default() -> Self {
                let mut __s = Self {
                    __arm: ::core::option::Option::None,
                    __default: ::core::option::Option::None,
                    #(#slot_news)*
                };
                #(#slot_defaults)*
                __s
            }
        }

        impl ::core::fmt::Debug for __Fill {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(stringify!(#name)).finish_non_exhaustive()
            }
        }

        impl __cb::Inspect for __Fill {
            fn at(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Datum> {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    return ::core::option::Option::None;
                };
                let __name = __first.name();
                if *__name == *#tag_text {
                    return if __rest.is_empty() {
                        self.__arm.map(|a| {
                            __cb::Datum::new("String", Self::__arm_name(a))
                        })
                    } else {
                        ::core::option::Option::None
                    };
                }
                let __arm = self.__arm.or(self.__default);
                #(#fill_at_fields)*
                ::core::option::Option::None
            }

            fn entries(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Entries> {
                let __arm = self.__arm.or(self.__default);
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    self.__arm?;
                    let mut __keys: __cb::__private::Vec<__cb::__private::String> =
                        __cb::__private::Vec::new();
                    __keys.push(__cb::__private::String::from(#tag_text));
                    #(#fill_keys)*
                    return ::core::option::Option::Some(__cb::Entries::Keys(__keys));
                };
                let __name = __first.name();
                #(#fill_entries_fields)*
                ::core::option::Option::None
            }
        }

        impl __cb::Fill<#name> for __Fill {
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
                let __name = __first.name();
                if *__name == *#tag_text {
                    if let ::core::option::Option::Some(__next) = __rest.first() {
                        return ::core::result::Result::Err(
                            __cb::FillError::unknown_key(__next.clone()).under(__first.clone()),
                        );
                    }
                    return self.__select(__text).map_err(|e| e.under(__first.clone()));
                }
                let mut __accepted = false;
                let mut __failure: ::core::option::Option<__cb::FillError> = ::core::option::Option::None;
                #(#set_fields)*
                if __accepted {
                    ::core::result::Result::Ok(())
                } else {
                    ::core::result::Result::Err(
                        __failure.unwrap_or_else(|| __cb::FillError::unknown_key(__first.clone())),
                    )
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
                let __name = __first.name();
                let mut __accepted = false;
                let mut __failure: ::core::option::Option<__cb::FillError> = ::core::option::Option::None;
                #(#text_fields)*
                if __accepted {
                    ::core::result::Result::Ok(())
                } else {
                    ::core::result::Result::Err(
                        __failure.unwrap_or_else(|| __cb::FillError::unknown_key(__first.clone())),
                    )
                }
            }

            fn clear(&mut self, __at: &[__cb::Segment]) -> ::core::result::Result<(), __cb::FillError> {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    return ::core::result::Result::Err(__cb::FillError::new(
                        __cb::fill::FillErrorKind::Unsupported("a group is cleared one key at a time"),
                    ));
                };
                let __name = __first.name();
                if *__name == *#tag_text {
                    self.__arm = ::core::option::Option::None;
                    return ::core::result::Result::Ok(());
                }
                let mut __accepted = false;
                #(#clear_fields)*
                if __accepted {
                    ::core::result::Result::Ok(())
                } else {
                    ::core::result::Result::Err(__cb::FillError::unknown_key(__first.clone()))
                }
            }

            fn set_default(&mut self, __value: #name) {
                match __value {
                    #(#default_arms)*
                }
            }

            #[allow(unused_variables)]
            fn build(
                self,
                __at: &__cb::KeyPath,
                __missing: &mut __cb::__private::Vec<__cb::KeyPath>,
            ) -> ::core::option::Option<#name> {
                let ::core::option::Option::Some(__arm) = self.__arm.or(self.__default) else {
                    __missing.push(__at.child(__cb::Segment::Key(__cb::__private::String::from(#tag_text))));
                    return ::core::option::Option::None;
                };
                match __arm {
                    #(#build_arms)*
                    _ => ::core::option::Option::None,
                }
            }

            fn put(&mut self, __value: #name) {
                match __value {
                    #(#put_arms)*
                }
            }

            fn put_any(
                &mut self,
                __at: &[__cb::Segment],
                __value: &mut dyn ::core::any::Any,
            ) -> ::core::result::Result<(), __cb::FillError> {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    return __cb::fill::put_whole::<#name, Self>(self, __value);
                };
                let __name = __first.name();
                if *__name == *#tag_text {
                    return ::core::result::Result::Err(__cb::FillError::new(
                        __cb::fill::FillErrorKind::Unsupported("the tag is set with text"),
                    ));
                }
                let mut __accepted = false;
                let mut __failure: ::core::option::Option<__cb::FillError> = ::core::option::Option::None;
                // The arm in force takes the value first; another arm with the key takes it
                // only when that one cannot.
                let __arm = self.__arm.or(self.__default);
                for __pass in 0..2 {
                    #(#any_fields)*
                    if __accepted {
                        break;
                    }
                }
                if __accepted {
                    ::core::result::Result::Ok(())
                } else {
                    ::core::result::Result::Err(
                        __failure.unwrap_or_else(|| __cb::FillError::unknown_key(__first.clone())),
                    )
                }
            }

            fn holds_none(&self, __at: &[__cb::Segment]) -> bool {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    return false;
                };
                let __name = __first.name();
                let __arm = self.__arm.or(self.__default);
                #(#none_fields)*
                false
            }

            fn from_value(__value: #name) -> Self {
                let mut __s = Self::default();
                match __value {
                    #(#value_arms)*
                }
                __s
            }
        }

        #[automatically_derived]
        impl __cb::Describe for #name #where_b {
            type Fill = __Fill;
            fn description() -> __cb::Description {
                let mut __d = __cb::Description::variant(__cb::describe::Variant::new(
                    #tag_text,
                    __cb::__private::vec![#(#arm_exprs),*],
                ));
                #doc
                __d
            }
        }

        #[automatically_derived]
        impl __cb::Inspect for #name {
            fn at(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Datum> {
                let __arm_name: &'static str = match self {
                    #(#built_name)*
                };
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    return ::core::option::Option::None;
                };
                let __name = __first.name();
                if *__name == *#tag_text {
                    return __rest.is_empty().then(|| __cb::Datum::new("String", __arm_name));
                }
                match self {
                    #(#built_at)*
                }
            }

            fn entries(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Entries> {
                let ::core::option::Option::Some((__first, __rest)) = __at.split_first() else {
                    let mut __keys: __cb::__private::Vec<__cb::__private::String> =
                        __cb::__private::Vec::new();
                    __keys.push(__cb::__private::String::from(#tag_text));
                    match self {
                        #(#built_keys)*
                    }
                    return ::core::option::Option::Some(__cb::Entries::Keys(__keys));
                };
                let __name = __first.name();
                match self {
                    #(#built_entries)*
                }
            }
        }
    })
}
