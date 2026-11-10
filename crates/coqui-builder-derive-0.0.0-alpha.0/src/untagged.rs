// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An untagged enum that holds a value: its text arms (unit names and single-value arms) are
//! one leaf, and at most one other arm is the structured side.
//!
//! A derive cannot see whether a held type is one value, so each arm asks its type
//! (`Describe::LEAF`) and the structured-arm count is a `const` assertion, which fails the
//! build of the enum rather than a run.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Ident, Type};

/// A variant `Name(T)` of an untagged enum.
pub(crate) struct Held {
    pub(crate) ident: Ident,
    pub(crate) ty: Type,
}

/// A unit variant: its identifier, the text it is stored as, and its label.
pub(crate) type Named = (Ident, String, Option<String>);

pub(crate) fn expand(
    name: &Ident,
    names: &[Named],
    held: &[Held],
    labels: &TokenStream,
) -> TokenStream {
    let ty_name = name.to_string();
    let slot = |i: usize| format_ident!("__a{}", i);
    let slots: Vec<Ident> = (0..held.len()).map(slot).collect();
    let tys: Vec<&Type> = held.iter().map(|h| &h.ty).collect();
    let unit_names: Vec<&String> = names.iter().map(|(_, n, _)| n).collect();

    let slot_decls = slots
        .iter()
        .zip(&tys)
        .map(|(s, ty)| quote!(#s: <#ty as __cb::Describe>::Fill,));
    let slot_news = slots
        .iter()
        .map(|s| quote!(#s: ::core::default::Default::default(),));

    // The structured-arm count and the nesting rule, checked when the enum is compiled.
    let counted = tys
        .iter()
        .map(|ty| quote!(+ (!<#ty as __cb::Describe>::LEAF) as usize));
    let nested = tys.iter().map(|ty| {
        quote! {
            ::core::assert!(
                !<#ty as __cb::Describe>::MIXED,
                ::core::concat!(
                    "`", ::core::stringify!(#name), "`: the structured arm of an untagged enum is a \
                     group, a list, a map or a tagged enum, not another untagged enum that \
                     holds a structured arm"
                )
            );
        }
    });
    let optional = tys.iter().map(|ty| {
        quote! {
            ::core::assert!(
                !<#ty as __cb::Describe>::OPTIONAL,
                ::core::concat!(
                    "`", ::core::stringify!(#name), "`: an untagged enum's arm holds a value; make the field that holds the enum optional instead"
                )
            );
        }
    });
    let unit_count = names.len();
    let text_arms = tys
        .iter()
        .map(|ty| quote!(+ (<#ty as __cb::Describe>::LEAF) as usize));
    let assertion = quote! {
        const _: () = {
            #(#optional)*
            let __texts = #unit_count #(#text_arms)*;
            ::core::assert!(
                __texts > 0,
                ::core::concat!(
                    "`", ::core::stringify!(#name), "`: an untagged enum with one arm is that arm: use a newtype"
                )
            );
            let __structured = 0usize #(#counted)*;
            ::core::assert!(
                __structured <= 1,
                ::core::concat!(
                    "`", ::core::stringify!(#name), "`: an untagged enum holds at most one arm that is \
                     not a single value; tell the others apart with a tag"
                )
            );
            #(#nested)*
        };
    };

    // Every arm that is a single value is tried by its own parser, in declaration order.
    let name_matches = names
        .iter()
        .map(|(i, n, _)| quote!(#n => return ::core::result::Result::Ok(#name::#i),));
    let unit_switch = if names.is_empty() {
        quote!()
    } else {
        quote!(match __text { #(#name_matches)* _ => {} })
    };
    let text_held = held.iter().map(|h| {
        let (i, ty) = (&h.ident, &h.ty);
        quote! {
            if <#ty as __cb::Describe>::LEAF {
                if let ::core::option::Option::Some(__v) = __cb::__private::parse_arm::<#ty>(__text) {
                    return ::core::result::Result::Ok(#name::#i(__v));
                }
            }
        }
    });
    let to_units = names
        .iter()
        .map(|(i, n, _)| quote!(#name::#i => __cb::__private::String::from(#n),));
    let to_held = held.iter().map(|h| {
        let (i, ty) = (&h.ident, &h.ty);
        quote! {
            #name::#i(__v) => {
                if <#ty as __cb::Describe>::LEAF {
                    __cb::__private::arm_text::<#ty>(__v)
                } else {
                    __cb::__private::String::new()
                }
            }
        }
    });
    let any_text_arm = quote!(false #(|| <#tys as __cb::Describe>::LEAF)*);
    let choices = if names.is_empty() {
        quote!(::core::option::Option::None)
    } else {
        quote! {
            if #any_text_arm {
                ::core::option::Option::None
            } else {
                ::core::option::Option::Some(&[#(#unit_names),*])
            }
        }
    };

    // Which slot is the structured arm: the one whose type is not a single value.
    let route = |call: &dyn Fn(&Ident, &Type) -> TokenStream| -> Vec<TokenStream> {
        held.iter()
            .enumerate()
            .map(|(i, h)| {
                let (s, ty) = (slot(i), &h.ty);
                let call = call(&s, ty);
                quote! {
                    if !<#ty as __cb::Describe>::LEAF {
                        __routed = ::core::option::Option::Some(#call);
                    }
                }
            })
            .collect()
    };
    let set_routes =
        route(&|s, ty| quote!(<_ as __cb::Fill<#ty>>::set(&mut self.#s, __at, __text)));
    let items_routes =
        route(&|s, ty| quote!(<_ as __cb::Fill<#ty>>::set_items(&mut self.#s, __at, __items)));
    let clear_routes = route(&|s, ty| quote!(<_ as __cb::Fill<#ty>>::clear(&mut self.#s, __at)));

    let drops = held.iter().enumerate().map(|(i, h)| {
        let (s, ty) = (slot(i), &h.ty);
        quote! {
            if !<#ty as __cb::Describe>::LEAF {
                __cb::__private::forget::<#ty, <#ty as __cb::Describe>::Fill>(&mut self.#s);
            }
        }
    });
    let inspect_at = held.iter().enumerate().map(|(i, h)| {
        let (s, ty) = (slot(i), &h.ty);
        quote! {
            if !<#ty as __cb::Describe>::LEAF {
                return __cb::Inspect::at(&self.#s, __at);
            }
        }
    });
    let inspect_entries = held.iter().enumerate().map(|(i, h)| {
        let (s, ty) = (slot(i), &h.ty);
        quote! {
            if !<#ty as __cb::Describe>::LEAF {
                return __cb::Inspect::entries(&self.#s, __at).or_else(|| {
                    __at.is_empty().then(|| {
                        __cb::Entries::Keys(__cb::__private::Vec::new())
                    })
                });
            }
        }
    });

    let put_units = names.iter().map(|(i, _, _)| {
        quote! {
            #name::#i => {
                self.__text = ::core::option::Option::Some(#name::#i);
                self.__drop_structured();
            }
        }
    });
    let put_held = held.iter().enumerate().map(|(i, h)| {
        let (s, ty, id) = (slot(i), &h.ty, &h.ident);
        quote! {
            #name::#id(__v) => {
                if <#ty as __cb::Describe>::LEAF {
                    self.__text = ::core::option::Option::Some(#name::#id(__v));
                    self.__drop_structured();
                } else {
                    <_ as __cb::Fill<#ty>>::put(&mut self.#s, __v);
                    self.__text = ::core::option::Option::None;
                    self.__structured = true;
                }
            }
        }
    });
    let any_text = held.iter().map(|h| {
        let (ty, id) = (&h.ty, &h.ident);
        quote! {
            if <#ty as __cb::Describe>::LEAF {
                if let ::core::option::Option::Some(__v) = __value
                    .downcast_mut::<::core::option::Option<#ty>>()
                    .and_then(::core::option::Option::take)
                {
                    <Self as __cb::Fill<#name>>::put(self, #name::#id(__v));
                    return ::core::result::Result::Ok(());
                }
            }
        }
    });
    let any_structured = held.iter().enumerate().map(|(i, h)| {
        let (s, ty) = (slot(i), &h.ty);
        quote! {
            if !<#ty as __cb::Describe>::LEAF {
                __routed = ::core::option::Option::Some(
                    <_ as __cb::Fill<#ty>>::put_any(&mut self.#s, __at, __value),
                );
            }
        }
    });
    let none_routes = held.iter().enumerate().map(|(i, h)| {
        let (s, ty) = (slot(i), &h.ty);
        quote! {
            if !<#ty as __cb::Describe>::LEAF {
                return <_ as __cb::Fill<#ty>>::holds_none(&self.#s, __at);
            }
        }
    });

    let default_units = names.iter().map(|(i, _, _)| {
        quote! {
            #name::#i => {
                self.__default = ::core::option::Option::Some(#name::#i);
                self.__structured_default = false;
            }
        }
    });
    let default_held = held.iter().enumerate().map(|(i, h)| {
        let (s, ty, id) = (slot(i), &h.ty, &h.ident);
        quote! {
            #name::#id(__v) => {
                if <#ty as __cb::Describe>::LEAF {
                    self.__default = ::core::option::Option::Some(#name::#id(__v));
                    self.__structured_default = false;
                } else {
                    <_ as __cb::Fill<#ty>>::set_default(&mut self.#s, __v);
                    self.__default = ::core::option::Option::None;
                    self.__structured_default = true;
                }
            }
        }
    });
    let build_arms = held.iter().enumerate().map(|(i, h)| {
        let (s, ty, id) = (slot(i), &h.ty, &h.ident);
        quote! {
            if !<#ty as __cb::Describe>::LEAF {
                return <_ as __cb::Fill<#ty>>::build(self.#s, __at, __missing).map(#name::#id);
            }
        }
    });
    let value_units = names.iter().map(
        |(i, _, _)| quote!(#name::#i => { __s.__text = ::core::option::Option::Some(#name::#i); }),
    );
    let value_held = held.iter().enumerate().map(|(i, h)| {
        let (s, ty, id) = (slot(i), &h.ty, &h.ident);
        quote! {
            #name::#id(__v) => {
                if <#ty as __cb::Describe>::LEAF {
                    __s.__text = ::core::option::Option::Some(#name::#id(__v));
                } else {
                    __s.#s = <<#ty as __cb::Describe>::Fill as __cb::Fill<#ty>>::from_value(__v);
                    __s.__structured = true;
                }
            }
        }
    });
    let structured_description = held.iter().map(|h| {
        let ty = &h.ty;
        quote! {
            if !<#ty as __cb::Describe>::LEAF {
                __structured = ::core::option::Option::Some(<#ty as __cb::Describe>::description());
            }
        }
    });
    let built_at = held.iter().map(|h| {
        let (id, ty) = (&h.ident, &h.ty);
        quote! {
            #name::#id(__v) => {
                if <#ty as __cb::Describe>::LEAF {
                    __at.is_empty().then(|| {
                        __cb::Datum::new(#ty_name, __cb::__private::arm_text::<#ty>(__v))
                    })
                } else {
                    __cb::Inspect::at(__v, __at)
                }
            }
        }
    });
    let built_entries = held.iter().map(|h| {
        let (id, ty) = (&h.ident, &h.ty);
        quote! {
            #name::#id(__v) => {
                if <#ty as __cb::Describe>::LEAF {
                    ::core::option::Option::None
                } else {
                    __cb::Inspect::entries(__v, __at)
                }
            }
        }
    });
    let built_unit_at = names.iter().map(|(i, n, _)| {
        quote! {
            #name::#i => __at
                .is_empty()
                .then(|| __cb::Datum::new(#ty_name, #n)),
        }
    });
    let built_unit_entries = names
        .iter()
        .map(|(i, _, _)| quote!(#name::#i => ::core::option::Option::None,));
    let all_leaf = quote!(true #(&& <#tys as __cb::Describe>::LEAF)*);

    quote! {
        #assertion

        #[doc(hidden)]
        pub struct __Text;

        impl __cb::describe::Codec<#name> for __Text {
            const TYPE: &'static str = #ty_name;
            #labels

            fn to_text(__value: &#name) -> __cb::__private::String {
                match __value {
                    #(#to_units)*
                    #(#to_held)*
                }
            }

            fn from_text(__text: &str) -> ::core::result::Result<#name, __cb::describe::LeafError> {
                #unit_switch
                #(#text_held)*
                ::core::result::Result::Err(__cb::describe::LeafError::Invalid {
                    type_name: #ty_name,
                })
            }

            fn choices() -> ::core::option::Option<&'static [&'static str]> {
                #choices
            }
        }

        #[doc(hidden)]
        pub struct __Fill {
            __text: ::core::option::Option<#name>,
            __default: ::core::option::Option<#name>,
            __structured: bool,
            __structured_default: bool,
            #(#slot_decls)*
        }

        impl __Fill {
            fn __drop_structured(&mut self) {
                if self.__structured {
                    #(#drops)*
                    self.__structured = false;
                }
            }
        }

        impl ::core::default::Default for __Fill {
            fn default() -> Self {
                Self {
                    __text: ::core::option::Option::None,
                    __default: ::core::option::Option::None,
                    __structured: false,
                    __structured_default: false,
                    #(#slot_news)*
                }
            }
        }

        impl ::core::fmt::Debug for __Fill {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(stringify!(#name)).finish_non_exhaustive()
            }
        }

        impl __cb::Inspect for __Fill {
            fn at(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Datum> {
                if __at.is_empty() {
                    return self.__text.as_ref().map(|__v| {
                        __cb::Datum::new(
                            #ty_name,
                            <__Text as __cb::describe::Codec<#name>>::to_text(__v),
                        )
                    });
                }
                if !self.__structured {
                    return ::core::option::Option::None;
                }
                #(#inspect_at)*
                ::core::option::Option::None
            }

            fn entries(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Entries> {
                if !self.__structured {
                    return ::core::option::Option::None;
                }
                #(#inspect_entries)*
                ::core::option::Option::None
            }
        }

        impl __cb::Fill<#name> for __Fill {
            fn set(
                &mut self,
                __at: &[__cb::Segment],
                __text: &str,
            ) -> ::core::result::Result<(), __cb::FillError> {
                let ::core::option::Option::Some(__first) = __at.first() else {
                    let __value = <__Text as __cb::describe::Codec<#name>>::from_text(__text)
                        .map_err(__cb::FillError::leaf)?;
                    self.__text = ::core::option::Option::Some(__value);
                    self.__drop_structured();
                    return ::core::result::Result::Ok(());
                };
                let mut __routed: ::core::option::Option<
                    ::core::result::Result<(), __cb::FillError>,
                > = ::core::option::Option::None;
                #(#set_routes)*
                match __routed {
                    ::core::option::Option::Some(::core::result::Result::Ok(())) => {
                        self.__text = ::core::option::Option::None;
                        self.__structured = true;
                        ::core::result::Result::Ok(())
                    }
                    ::core::option::Option::Some(::core::result::Result::Err(e)) => {
                        ::core::result::Result::Err(e)
                    }
                    ::core::option::Option::None => {
                        ::core::result::Result::Err(__cb::FillError::unknown_key(__first.clone()))
                    }
                }
            }

            fn set_items(
                &mut self,
                __at: &[__cb::Segment],
                __items: &[__cb::__private::String],
            ) -> ::core::result::Result<(), __cb::FillError> {
                let mut __routed: ::core::option::Option<
                    ::core::result::Result<(), __cb::FillError>,
                > = ::core::option::Option::None;
                #(#items_routes)*
                match __routed {
                    ::core::option::Option::Some(::core::result::Result::Ok(())) => {}
                    // An empty table chooses a group or a tagged enum, which hold no items.
                    ::core::option::Option::Some(::core::result::Result::Err(e))
                        if __at.is_empty()
                            && __items.is_empty()
                            && ::core::matches!(
                                e.kind(),
                                __cb::fill::FillErrorKind::Unsupported(_)
                            ) => {}
                    ::core::option::Option::Some(::core::result::Result::Err(e)) => {
                        return ::core::result::Result::Err(e);
                    }
                    ::core::option::Option::None => {
                        return ::core::result::Result::Err(match __at.first() {
                            ::core::option::Option::Some(__first) => {
                                __cb::FillError::unknown_key(__first.clone())
                            }
                            ::core::option::Option::None => __cb::FillError::new(
                                __cb::fill::FillErrorKind::Unsupported("a text is not a list"),
                            ),
                        });
                    }
                }
                self.__text = ::core::option::Option::None;
                self.__structured = true;
                ::core::result::Result::Ok(())
            }

            fn clear(&mut self, __at: &[__cb::Segment]) -> ::core::result::Result<(), __cb::FillError> {
                let ::core::option::Option::Some(__first) = __at.first() else {
                    self.__text = ::core::option::Option::None;
                    self.__drop_structured();
                    return ::core::result::Result::Ok(());
                };
                let mut __routed: ::core::option::Option<
                    ::core::result::Result<(), __cb::FillError>,
                > = ::core::option::Option::None;
                #(#clear_routes)*
                __routed.unwrap_or_else(|| {
                    ::core::result::Result::Err(__cb::FillError::unknown_key(__first.clone()))
                })
            }

            fn set_default(&mut self, __value: #name) {
                match __value {
                    #(#default_units)*
                    #(#default_held)*
                }
            }

            #[allow(unused_variables)]
            fn build(
                self,
                __at: &__cb::KeyPath,
                __missing: &mut __cb::__private::Vec<__cb::KeyPath>,
            ) -> ::core::option::Option<#name> {
                if self.__structured || (self.__text.is_none() && self.__structured_default) {
                    #(#build_arms)*
                    return ::core::option::Option::None;
                }
                if let ::core::option::Option::Some(__v) = self.__text.or(self.__default) {
                    return ::core::option::Option::Some(__v);
                }
                __missing.push(::core::clone::Clone::clone(__at));
                ::core::option::Option::None
            }

            fn put(&mut self, __value: #name) {
                match __value {
                    #(#put_units)*
                    #(#put_held)*
                }
            }

            fn put_any(
                &mut self,
                __at: &[__cb::Segment],
                __value: &mut dyn ::core::any::Any,
            ) -> ::core::result::Result<(), __cb::FillError> {
                if __at.is_empty() {
                    if __value.is::<::core::option::Option<#name>>() {
                        return __cb::fill::put_whole::<#name, Self>(self, __value);
                    }
                    #(#any_text)*
                }
                let mut __routed: ::core::option::Option<
                    ::core::result::Result<(), __cb::FillError>,
                > = ::core::option::Option::None;
                #(#any_structured)*
                match __routed {
                    ::core::option::Option::Some(::core::result::Result::Ok(())) => {
                        self.__text = ::core::option::Option::None;
                        self.__structured = true;
                        ::core::result::Result::Ok(())
                    }
                    ::core::option::Option::Some(::core::result::Result::Err(e)) => {
                        ::core::result::Result::Err(e)
                    }
                    ::core::option::Option::None => ::core::result::Result::Err(match __at.first() {
                        ::core::option::Option::Some(__first) => {
                            __cb::FillError::unknown_key(__first.clone())
                        }
                        ::core::option::Option::None => __cb::FillError::new(
                            __cb::fill::FillErrorKind::Unsupported(
                                "the value is not of the type of this setting",
                            ),
                        ),
                    }),
                }
            }

            fn holds_none(&self, __at: &[__cb::Segment]) -> bool {
                if !self.__structured {
                    return false;
                }
                #(#none_routes)*
                false
            }

            fn from_value(__value: #name) -> Self {
                let mut __s = Self::default();
                match __value {
                    #(#value_units)*
                    #(#value_held)*
                }
                __s
            }
        }

        #[automatically_derived]
        impl __cb::Describe for #name {
            type Fill = __Fill;
            const LEAF: bool = #all_leaf;
            const MIXED: bool = !<Self as __cb::Describe>::LEAF;

            fn description() -> __cb::Description {
                let __text = __cb::describe::LeafInfo::of_codec::<#name, __Text>();
                let mut __structured: ::core::option::Option<__cb::Description> =
                    ::core::option::Option::None;
                #(#structured_description)*
                match __structured {
                    ::core::option::Option::Some(__s) => __cb::Description::union(__text, __s),
                    ::core::option::Option::None => {
                        __cb::Description::new(__cb::describe::Kind::Leaf(__text))
                    }
                }
            }
        }

        #[automatically_derived]
        impl __cb::Inspect for #name {
            fn at(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Datum> {
                match self {
                    #(#built_unit_at)*
                    #(#built_at)*
                }
            }

            fn entries(&self, __at: &[__cb::Segment]) -> ::core::option::Option<__cb::Entries> {
                match self {
                    #(#built_unit_entries)*
                    #(#built_entries)*
                }
            }
        }
    }
}
