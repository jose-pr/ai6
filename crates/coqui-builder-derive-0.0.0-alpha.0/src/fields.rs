// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What one named field becomes: its key, its slot, its description and how it is read.

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Field, Ident, Type};

use crate::attrs::{DefaultKind, FieldAttrs, option_inner, vec_inner};
use crate::rename::RenameRule;

/// How the field becomes text and back.
// One value per field of a type being derived, at build time: boxing would only add code.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Shape {
    /// The field's type is `Describe` itself.
    Plain,
    /// A codec converts the field (`with`, `as_text`).
    Codec {
        codec: TokenStream,
        item: Type,
        wrap: Wrap,
    },
}

/// What surrounds the converted item.
#[derive(Clone, Copy)]
pub(crate) enum Wrap {
    One,
    Opt,
    List,
    OptList,
}

/// One field, read once and shared by every impl.
pub(crate) struct FieldPlan {
    pub(crate) ident: Ident,
    pub(crate) ty: Type,
    pub(crate) key: String,
    pub(crate) attrs: FieldAttrs,
    pub(crate) shape: Shape,
    /// The inner type of `Option<_>`, when the field is one.
    pub(crate) option: Option<Type>,
    /// A codec to define beside the impls, when the field is `with` or `as_text`.
    pub(crate) codec_def: Option<TokenStream>,
    /// The field's doc comment, as written.
    pub(crate) docs: Vec<syn::Attribute>,
}

/// The last segment of a type's path, as the name a datum of it shows.
fn type_name(ty: &Type) -> String {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map_or_else(|| "Text".into(), |s| s.ident.to_string()),
        _ => "Text".into(),
    }
}

impl FieldPlan {
    pub(crate) fn read(
        field: &Field,
        index: usize,
        rule: Option<RenameRule>,
        owner: &Ident,
    ) -> syn::Result<FieldPlan> {
        let ident = field.ident.clone().expect("a named field");
        let attrs = FieldAttrs::read(field)?;
        if !attrs.skip && attrs.with.is_none() && !attrs.as_text {
            refuse_self_reference(&field.ty, owner)?;
        }
        let key = attrs.rename.clone().unwrap_or_else(|| {
            let name = ident.unraw().to_string();
            rule.map_or(name.clone(), |rule| rule.field(&name))
        });
        let option = option_inner(&field.ty).cloned();
        let mut codec_def = None;
        let shape = if let Some(path) = &attrs.with {
            let codec = format_ident!("__With{}", index);
            let ty = &field.ty;
            codec_def = Some(quote! {
                struct #codec;
                impl __cb::describe::Codec<#ty> for #codec {
                    const TYPE: &'static str = #path::TYPE;
                    fn to_text(value: &#ty) -> __cb::__private::String {
                        #path::to_text(value)
                    }
                    fn from_text(text: &str) -> ::core::result::Result<#ty, __cb::describe::LeafError> {
                        #path::from_text(text)
                    }
                }
            });
            Shape::Codec {
                codec: quote!(#codec),
                item: field.ty.clone(),
                wrap: Wrap::One,
            }
        } else if attrs.as_text {
            let (item, wrap) = match &option {
                Some(inner) => match vec_inner(inner) {
                    Some(item) => (item.clone(), Wrap::OptList),
                    None => (inner.clone(), Wrap::Opt),
                },
                None => match vec_inner(&field.ty) {
                    Some(item) => (item.clone(), Wrap::List),
                    None => (field.ty.clone(), Wrap::One),
                },
            };
            let codec = format_ident!("__AsText{}", index);
            let name = type_name(&item);
            codec_def = Some(quote! {
                struct #codec;
                impl __cb::describe::Codec<#item> for #codec {
                    const TYPE: &'static str = #name;
                    fn to_text(value: &#item) -> __cb::__private::String {
                        __cb::__private::ToString::to_string(value)
                    }
                    fn from_text(text: &str) -> ::core::result::Result<#item, __cb::describe::LeafError> {
                        text.parse::<#item>().map_err(|_| __cb::describe::LeafError::Invalid {
                            type_name: #name,
                        })
                    }
                }
            });
            Shape::Codec {
                codec: quote!(#codec),
                item,
                wrap,
            }
        } else {
            Shape::Plain
        };
        Ok(FieldPlan {
            ident,
            ty: field.ty.clone(),
            key,
            attrs,
            shape,
            option,
            codec_def,
            docs: field
                .attrs
                .iter()
                .filter(|a| a.path().is_ident("doc"))
                .cloned()
                .collect(),
        })
    }

    pub(crate) fn skipped(&self) -> bool {
        self.attrs.skip
    }

    pub(crate) fn flatten(&self) -> bool {
        self.attrs.flatten
    }

    /// The slot that fills the field.
    pub(crate) fn slot(&self) -> TokenStream {
        let ty = &self.ty;
        match &self.shape {
            Shape::Plain => quote!(<#ty as __cb::Describe>::Fill),
            Shape::Codec { codec, item, wrap } => {
                let leaf = quote!(__cb::fill::LeafSlot<#item, #codec>);
                match wrap {
                    Wrap::One => leaf,
                    Wrap::Opt => quote!(__cb::fill::OptionSlot<#item, #leaf>),
                    Wrap::List => quote!(__cb::fill::ListSlot<#item, #leaf>),
                    Wrap::OptList => quote!(
                        __cb::fill::OptionSlot<
                            __cb::__private::Vec<#item>,
                            __cb::fill::ListSlot<#item, #leaf>,
                        >
                    ),
                }
            }
        }
    }

    /// A new, empty slot for the field (with the text that writes `None`, when declared).
    pub(crate) fn new_slot(&self) -> TokenStream {
        let slot = self.slot();
        match (&self.attrs.none, &self.option) {
            (Some(none), Some(inner)) if matches!(self.shape, Shape::Plain) => quote! {
                <__cb::fill::OptionSlot<#inner, <#inner as __cb::Describe>::Fill>>::default()
                    .none(<#inner as __cb::Leaf>::TYPE, #none)
            },
            _ => quote!(<#slot as ::core::default::Default>::default()),
        }
    }

    /// The slot filled from `value`, keeping the text that writes `None`.
    pub(crate) fn slot_from_value(&self, value: TokenStream) -> TokenStream {
        let ty = &self.ty;
        let slot = self.slot();
        let built = quote!(<#slot as __cb::Fill<#ty>>::from_value(#value));
        match (&self.attrs.none, &self.option) {
            (Some(none), Some(inner)) if matches!(self.shape, Shape::Plain) => quote! {
                <__cb::fill::OptionSlot<#inner, <#inner as __cb::Describe>::Fill>>::none(
                    #built, <#inner as __cb::Leaf>::TYPE, #none
                )
            },
            _ => built,
        }
    }

    /// The description of the field's value.
    fn description(&self) -> TokenStream {
        let ty = &self.ty;
        match &self.shape {
            Shape::Plain => quote!(<#ty as __cb::Describe>::description()),
            Shape::Codec { codec, item, wrap } => {
                let leaf = quote!(__cb::Description::leaf_of::<#item, #codec>());
                match wrap {
                    Wrap::List | Wrap::OptList => quote!(__cb::Description::list(#leaf)),
                    _ => leaf,
                }
            }
        }
    }

    fn optional(&self) -> TokenStream {
        let ty = &self.ty;
        match &self.shape {
            Shape::Plain => quote!(<#ty as __cb::Describe>::OPTIONAL),
            Shape::Codec { wrap, .. } => match wrap {
                Wrap::Opt | Wrap::OptList => quote!(true),
                _ => quote!(false),
            },
        }
    }

    /// An expression of type `Option<String>` for `value` (an expression of the field's type,
    /// borrowed), as the text its leaf would give.
    fn text_of(&self, value: TokenStream) -> TokenStream {
        let ty = &self.ty;
        match &self.shape {
            Shape::Plain => quote! {
                <#ty as __cb::Inspect>::at(#value, &[]).map(|d| __cb::__private::String::from(d.text()))
            },
            Shape::Codec { codec, item, wrap } => {
                let call = match wrap {
                    Wrap::One => quote!(at_one),
                    Wrap::Opt => quote!(at_opt),
                    Wrap::List => quote!(at_list),
                    Wrap::OptList => quote!(at_opt_list),
                };
                let rest = match wrap {
                    Wrap::List | Wrap::OptList => quote!(&[__cb::Segment::Index(0)]),
                    _ => quote!(&[]),
                };
                quote! {
                    __cb::__private::#call::<#item, #codec>(#value, #rest)
                        .map(|d| __cb::__private::String::from(d.text()))
                }
            }
        }
    }

    /// The `Field` for this field, as an expression.
    pub(crate) fn field_expr(&self) -> TokenStream {
        let key = &self.key;
        let description = self.description();
        let optional = self.optional();
        let notes = self.notes();
        let none = self.attrs.none.as_ref().map(|none| {
            let inner = self.option.as_ref();
            quote! {
                __f.none = ::core::option::Option::Some(__cb::__private::String::from(#none));
                debug_assert!(
                    <#inner as __cb::Leaf>::from_text(#none).is_err(),
                    "the text that writes None is also a spelling of the type"
                );
            }
        });
        let default = self.default_stmt();
        let stricter = self.attrs.stricter.map(|higher| {
            let way = if higher {
                quote!(Higher)
            } else {
                quote!(Lower)
            };
            quote! { __d.notes_mut().stricter = ::core::option::Option::Some(__cb::describe::Stricter::#way); }
        });
        quote! {
            {
                let mut __d = #description;
                #notes
                #stricter
                let mut __f = __cb::describe::Field::new(#key, __d);
                __f.optional = #optional;
                __f.required = !__f.optional;
                #none
                #default
                __f
            }
        }
    }

    fn default_stmt(&self) -> Option<TokenStream> {
        let expr = match self.attrs.default.as_ref()? {
            DefaultKind::Trait => {
                let ty = &self.ty;
                quote!(<#ty as ::core::default::Default>::default())
            }
            DefaultKind::Expr(expr) => quote!(#expr),
        };
        let ty = &self.ty;
        Some(match &self.shape {
            Shape::Plain => quote! {
                {
                    let __v: #ty = #expr;
                    __f.default_value::<#ty>(__v);
                }
            },
            Shape::Codec { codec, item, wrap } => {
                let call = match wrap {
                    Wrap::One => quote!(__f.default_leaf::<#item, #codec>(&__v)),
                    Wrap::Opt => quote!(__f.default_option::<#item, #codec>(&__v)),
                    Wrap::List => quote!(__f.default_list::<#item, #codec>(&__v)),
                    Wrap::OptList => quote! {
                        if let ::core::option::Option::Some(__l) = &__v {
                            __f.default_list::<#item, #codec>(__l);
                        }
                        __f.required = false;
                    },
                };
                quote! {
                    {
                        let __v: #ty = #expr;
                        #call;
                    }
                }
            }
        })
    }

    /// Statements setting the notes on `__d`.
    fn notes(&self) -> TokenStream {
        let s = &self.attrs.schema;
        let mut out = TokenStream::new();
        let some = |text: &String| quote!(::core::option::Option::Some(__cb::__private::String::from(#text)));
        if let Some(t) = &s.title {
            let v = some(t);
            out.extend(quote!(__n.title = #v;));
        }
        if let Some(t) = &s.description {
            let v = some(t);
            out.extend(quote!(__n.description = #v;));
        }
        if let Some(t) = &s.section {
            let v = some(t);
            out.extend(quote!(__n.section = #v;));
        }
        if let Some(n) = s.order {
            out.extend(quote!(__n.order = ::core::option::Option::Some(#n);));
        }
        if s.advanced {
            out.extend(quote!(__n.advanced = true;));
        }
        if s.sensitive {
            out.extend(quote!(__n.sensitive = true;));
        }
        if s.read_only {
            out.extend(quote!(__n.read_only = true;));
        }
        if s.deprecated {
            out.extend(quote!(__n.deprecated = true;));
        }
        if let Some(n) = s.min_length {
            out.extend(quote!(__n.min_length = ::core::option::Option::Some(#n);));
        }
        if let Some(n) = s.max_length {
            out.extend(quote!(__n.max_length = ::core::option::Option::Some(#n);));
        }
        if let Some(t) = &s.pattern {
            let v = some(t);
            out.extend(quote!(__n.pattern = #v;));
        }
        if let Some(t) = &s.format {
            let v = some(t);
            out.extend(quote!(__n.format = #v;));
        }
        if let Some(x) = s.multiple_of {
            out.extend(quote!(__n.multiple_of = ::core::option::Option::Some(#x);));
        }
        if let Some(n) = s.min_items {
            out.extend(quote!(__n.min_items = ::core::option::Option::Some(#n);));
        }
        if let Some(n) = s.max_items {
            out.extend(quote!(__n.max_items = ::core::option::Option::Some(#n);));
        }
        for example in &s.examples {
            let ty = &self.ty;
            let text = self.text_of(quote!(&__e));
            out.extend(quote! {
                {
                    let __e: #ty = #example;
                    if let ::core::option::Option::Some(__text) = #text {
                        __n.examples.push(__text);
                    }
                }
            });
        }
        if out.is_empty() {
            return out;
        }
        quote! { { let __n = __d.notes_mut(); #out } }
    }

    /// An expression answering `Inspect::at(rest)` for this field of a built value, where
    /// `value` is an expression of type `&FieldTy`.
    pub(crate) fn inspect_built(&self, value: TokenStream) -> TokenStream {
        match &self.shape {
            Shape::Plain => match (&self.attrs.none, &self.option) {
                (Some(none), Some(inner)) => quote! {
                    match #value {
                        ::core::option::Option::None => __rest.is_empty().then(|| {
                            __cb::Datum::new(<#inner as __cb::Leaf>::TYPE, #none)
                        }),
                        ::core::option::Option::Some(__v) => __cb::Inspect::at(__v, __rest),
                    }
                },
                _ => quote!(__cb::Inspect::at(#value, __rest)),
            },
            Shape::Codec { codec, item, wrap } => {
                let call = match wrap {
                    Wrap::One => quote!(at_one),
                    Wrap::Opt => quote!(at_opt),
                    Wrap::List => quote!(at_list),
                    Wrap::OptList => quote!(at_opt_list),
                };
                quote!(__cb::__private::#call::<#item, #codec>(#value, __rest))
            }
        }
    }

    /// An expression answering `Inspect::entries(rest)` for this field of a built value.
    pub(crate) fn entries_built(&self, value: TokenStream) -> TokenStream {
        match &self.shape {
            Shape::Plain => quote!(__cb::Inspect::entries(#value, __rest)),
            Shape::Codec { wrap, .. } => match wrap {
                Wrap::List => quote!(__cb::__private::entries_list(#value, __rest)),
                Wrap::OptList => quote!(__cb::__private::entries_opt_list(#value, __rest)),
                _ => quote!(::core::option::Option::<__cb::Entries>::None),
            },
        }
    }

    pub(crate) fn span(&self) -> proc_macro2::Span {
        self.ident.span()
    }

    pub(crate) fn flatten_bound(&self, root: &TokenStream) -> TokenStream {
        let ty = &self.ty;
        quote_spanned!(self.ty.span()=> #ty: #root::Fields)
    }
}

/// A type that holds itself has a description with no end.
pub(crate) fn refuse_self_reference(ty: &Type, owner: &Ident) -> syn::Result<()> {
    /// Whether `ty` names the owner as a bare path, itself or inside a generic argument,
    /// reference, array, slice or tuple; `other::Owner` is another type.
    fn names(ty: &Type, owner: &Ident) -> bool {
        match ty {
            Type::Path(path) => {
                let bare = path.qself.is_none() && path.path.leading_colon.is_none();
                if bare
                    && path
                        .path
                        .segments
                        .first()
                        .is_some_and(|s| s.ident == *owner || s.ident == "Self")
                {
                    return true;
                }
                path.path.segments.iter().any(|segment| {
                    matches!(&segment.arguments, syn::PathArguments::AngleBracketed(args)
                    if args.args.iter().any(|arg| {
                        matches!(arg, syn::GenericArgument::Type(inner) if names(inner, owner))
                    }))
                })
            }
            Type::Reference(r) => names(&r.elem, owner),
            Type::Slice(s) => names(&s.elem, owner),
            Type::Array(a) => names(&a.elem, owner),
            Type::Paren(p) => names(&p.elem, owner),
            Type::Group(g) => names(&g.elem, owner),
            Type::Tuple(t) => t.elems.iter().any(|e| names(e, owner)),
            _ => false,
        }
    }
    if names(ty, owner) {
        return Err(syn::Error::new_spanned(
            ty,
            format!(
                "`{owner}` holds itself through this field, and a description with no end \
                 cannot be built. Describe the recursion as a list of a separate type, or \
                 skip the field with `#[map(skip)]`."
            ),
        ));
    }
    Ok(())
}
