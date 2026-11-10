// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The derive's refusals and accepted spellings, held to `tests/vectors/attributes.json`.

use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use serde_json::Value;
use syn::{Attribute, Data, DeriveInput, Fields};

use crate::attrs::{
    BUILDER_FIELD_KEYS, BUILDER_TYPE_KEYS, MAP_ENUM_KEYS, MAP_FIELD_KEYS, MAP_STRUCT_KEYS,
    MAP_VARIANT_KEYS, RESERVED_SETTERS, SCHEMA_ARM_KEYS, SCHEMA_CHOICE_KEYS, SCHEMA_FIELD_KEYS,
};

fn vectors() -> Value {
    serde_json::from_str(include_str!("../tests/vectors/attributes.json")).expect("valid JSON")
}

/// The items a case derives, with the helper text before the first one dropped.
fn derived(item: &str) -> Vec<String> {
    item.split("{derive}").skip(1).map(str::to_owned).collect()
}

fn expand_text(item: &str) -> syn::Result<TokenStream> {
    let tokens: TokenStream = item.parse().expect("tokens");
    crate::try_expand(tokens)
}

fn cases() -> Vec<Value> {
    vectors()["cases"].as_array().expect("cases").clone()
}

#[test]
fn every_case_is_accepted_or_refused_as_the_file_says() {
    for case in cases() {
        let name = case["name"].as_str().expect("name");
        let items = derived(case["item"].as_str().expect("item"));
        assert!(!items.is_empty(), "{name}: no {{derive}} marker");
        let results: Vec<_> = items.iter().map(|i| expand_text(i)).collect();
        match &case["coqui_builder"] {
            Value::String(s) if s == "accept" => {
                for (item, result) in items.iter().zip(&results) {
                    let tokens = result
                        .as_ref()
                        .unwrap_or_else(|e| panic!("{name}: refused: {e}\n{item}"));
                    syn::parse2::<syn::File>(tokens.clone())
                        .unwrap_or_else(|e| panic!("{name}: the expansion is not Rust: {e}"));
                }
            }
            Value::Object(o) => {
                let stem = o["refuse"].as_str().expect("a stem");
                let refusals: Vec<String> = results
                    .iter()
                    .filter_map(|r| r.as_ref().err().map(ToString::to_string))
                    .collect();
                assert!(
                    refusals.iter().any(|m| m.contains(stem)),
                    "{name}: expected a refusal containing {stem:?}, got {refusals:?}"
                );
            }
            other => panic!("{name}: unreadable outcome {other:?}"),
        }
    }
}

#[derive(Default)]
struct Seen {
    map_struct: BTreeSet<String>,
    map_enum: BTreeSet<String>,
    map_field: BTreeSet<String>,
    map_variant: BTreeSet<String>,
    schema_field: BTreeSet<String>,
    schema_arm: BTreeSet<String>,
    schema_choice: BTreeSet<String>,
    builder_field: BTreeSet<String>,
    builder_type: BTreeSet<String>,
}

fn keys(attrs: &[Attribute], name: &str, into: &mut BTreeSet<String>) {
    for attr in attrs.iter().filter(|a| a.path().is_ident(name)) {
        attr.parse_nested_meta(|meta| {
            if let Some(ident) = meta.path.get_ident() {
                into.insert(ident.to_string());
            }
            if meta.input.peek(syn::Token![=]) {
                let _: syn::Expr = meta.value()?.parse()?;
            } else if meta.input.peek(syn::token::Paren) {
                let content;
                syn::parenthesized!(content in meta.input);
                let _: TokenStream = content.parse()?;
            }
            Ok(())
        })
        .expect("attributes parse");
    }
}

fn scan(ast: &DeriveInput, seen: &mut Seen) {
    let fields_of = |fields: &Fields, seen: &mut Seen| {
        for field in fields {
            keys(&field.attrs, "map", &mut seen.map_field);
            keys(&field.attrs, "schema", &mut seen.schema_field);
            keys(&field.attrs, "builder", &mut seen.builder_field);
        }
    };
    keys(&ast.attrs, "builder", &mut seen.builder_type);
    match &ast.data {
        Data::Struct(data) => {
            keys(&ast.attrs, "map", &mut seen.map_struct);
            fields_of(&data.fields, seen);
        }
        Data::Enum(data) => {
            keys(&ast.attrs, "map", &mut seen.map_enum);
            let mut own = BTreeSet::new();
            keys(&ast.attrs, "map", &mut own);
            let tagged = own.contains("tag");
            for variant in &data.variants {
                keys(&variant.attrs, "map", &mut seen.map_variant);
                if tagged {
                    keys(&variant.attrs, "schema", &mut seen.schema_arm);
                } else {
                    keys(&variant.attrs, "schema", &mut seen.schema_choice);
                }
                fields_of(&variant.fields, seen);
            }
        }
        Data::Union(_) => {}
    }
}

#[test]
fn the_key_lists_are_exactly_the_keys_the_accepted_cases_use() {
    let mut seen = Seen::default();
    for case in cases() {
        if case["coqui_builder"] != "accept" {
            continue;
        }
        for item in derived(case["item"].as_str().expect("item")) {
            let ast: DeriveInput = syn::parse_str(&item).unwrap_or_else(|e| panic!("{item}: {e}"));
            scan(&ast, &mut seen);
        }
    }
    let list =
        |keys: &[&str]| -> BTreeSet<String> { keys.iter().map(|k| (*k).to_owned()).collect() };
    for (what, got, want) in [
        (
            "#[map] on a struct",
            &seen.map_struct,
            list(MAP_STRUCT_KEYS),
        ),
        ("#[map] on an enum", &seen.map_enum, list(MAP_ENUM_KEYS)),
        ("#[map] on a field", &seen.map_field, list(MAP_FIELD_KEYS)),
        (
            "#[map] on a variant",
            &seen.map_variant,
            list(MAP_VARIANT_KEYS),
        ),
        (
            "#[schema] on a field",
            &seen.schema_field,
            list(SCHEMA_FIELD_KEYS),
        ),
        (
            "#[schema] on an arm",
            &seen.schema_arm,
            list(SCHEMA_ARM_KEYS),
        ),
        (
            "#[schema] on a choice",
            &seen.schema_choice,
            list(SCHEMA_CHOICE_KEYS),
        ),
        (
            "#[builder] on a field",
            &seen.builder_field,
            list(BUILDER_FIELD_KEYS),
        ),
        (
            "#[builder] on a type",
            &seen.builder_type,
            list(BUILDER_TYPE_KEYS),
        ),
    ] {
        assert_eq!(
            *got, want,
            "{what}: the accepted cases and the key list disagree"
        );
    }
}

#[test]
fn the_reserved_setter_names_are_the_ones_every_builder_has() {
    assert_eq!(
        RESERVED_SETTERS,
        [
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
            "try_into"
        ]
    );
}

#[test]
fn a_message_never_quotes_the_input_it_refused() {
    let err = expand_text("struct S { #[map(rename_all = \"x\")] a: u8 }").unwrap_err();
    assert!(!err.to_string().contains("rename_all = \"x\""));
}

#[test]
fn a_field_naming_the_owner_is_refused_unless_it_is_skipped_or_another_type() {
    let refused = |item: &str| expand_text(item).unwrap_err().to_string();
    assert!(
        refused("#[derive(Describe)] struct Node { next: Option<Box<Node>> }")
            .contains("holds itself")
    );
    assert!(
        refused("#[derive(Describe)] struct Node { next: Vec<Self> }").contains("holds itself")
    );
    for ok in [
        "#[derive(Describe)] struct Duration { d: core::time::Duration }",
        "#[derive(Describe)] struct Tls { inner: other::Tls }",
        "#[derive(Describe)] struct Node { #[map(skip)] parent: Option<Box<Node>> }",
    ] {
        assert!(expand_text(ok).is_ok(), "{ok}");
    }
}

/// The text of `tokens` with the whitespace and line continuations of its literals dropped.
fn squeezed(tokens: &str) -> String {
    tokens
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\\')
        .collect()
}

#[test]
fn an_untagged_enum_with_a_held_arm_asserts_its_structured_arm_count() {
    let held = expand_text("#[map(untagged)] enum E { Off, A(A), B(B) }")
        .expect("accepted: the count is checked when the enum compiles")
        .to_string();
    let message = "an untagged enum holds at most one arm that is not a single value; \
                   tell the others apart with a tag";
    assert!(squeezed(&held).contains(&squeezed(message)), "{held}");
    assert!(held.contains("MIXED"), "{held}");
    // Names alone stay a plain leaf, with no union machinery.
    let names = expand_text("#[map(untagged)] enum E { Off, On }")
        .expect("accepted")
        .to_string();
    assert!(!names.contains("__Fill"), "{names}");
}

#[test]
fn an_untagged_arm_that_holds_the_enum_is_refused() {
    let refused = expand_text("#[map(untagged)] enum Tree { Leaf(u8), Kids(Vec<Tree>) }")
        .expect_err("a description with no end")
        .to_string();
    assert!(refused.contains("holds itself"), "{refused}");
}

#[test]
fn an_untagged_enum_also_asserts_no_option_arm_and_at_least_one_text_arm() {
    let held = expand_text("#[map(untagged)] enum E { Off, A(A) }")
        .expect("accepted: the arms are checked when the enum compiles")
        .to_string();
    for message in [
        "an untagged enum's arm holds a value; make the field that holds the enum optional \
         instead",
        "an untagged enum with one arm is that arm: use a newtype",
    ] {
        assert!(
            squeezed(&held).contains(&squeezed(message)),
            "{message}: {held}"
        );
    }
}

/// The tokens `#[derive(Builder)]` gives for `item`, with whitespace and line continuations
/// dropped.
fn builder_text(item: &str) -> String {
    let tokens: TokenStream = item.parse().expect("tokens");
    squeezed(&crate::builder::expand(tokens).to_string())
}

#[test]
fn a_parts_struct_is_refused_with_a_sentence() {
    let settings = "struct P { settings: S, #[builder(input)] id: u8 }";
    let refused = |item: &str, stem: &str| {
        let text = builder_text(item);
        assert!(text.contains("compile_error"), "{item}: accepted");
        assert!(
            text.contains(&squeezed(stem)),
            "{item}: expected {stem:?}, got {text}"
        );
    };
    refused(
        "#[builder(build = X)] struct P { settings: S, other: S }",
        "has one settings field, and this is a second",
    );
    refused(
        "#[builder(build = X)] struct P { #[builder(input)] id: u8 }",
        "has one settings field, and this has none",
    );
    refused(settings, "needs `#[builder(build = X)]`");
    refused(
        &format!("#[builder(build = X, title = \"Y\")] {settings}"),
        "unrecognised `#[builder(...)]` option on a parts struct",
    );
    refused(
        "#[builder(build = X)] struct P { settings: S, #[builder(input, output)] id: u8 }",
        "unrecognised `#[builder(...)]` option on a field of a parts struct",
    );
    refused(
        &format!("#[builder(build = Built<u8>)] {settings}"),
        "Name the builder with `name",
    );
    refused(
        &format!("#[builder(build = X, name = \"not a name\")] {settings}"),
        "`name` is the builder's type name",
    );
    refused(
        "#[builder(build = X)] struct P { settings: S, #[builder(input)] build: u8 }",
        "is a method every builder has",
    );
    refused(
        "#[builder(build = X)] struct P { settings: S, #[builder(input)] id: u8, \
         #[builder(input, setter = \"id\")] other: u8 }",
        "both have the setter `id`",
    );
    refused(
        "#[builder(build = X)] struct P<'a> { settings: S, #[builder(input)] id: &'a u8 }",
        "needs `entry = \"new\"`",
    );
    refused(
        &format!("#[builder(build = X, entry = \"make\")] {settings}"),
        "`entry` is \"new\"",
    );
    refused(
        "#[builder(build = X)] struct P(S);",
        "needs a parts struct with named fields",
    );
    refused(
        "#[builder(build = X)] enum P { A }",
        "is for a parts struct",
    );
    refused(
        "#[builder(build = X)] struct P { #[builder(setter = \"s\")] settings: S }",
        "renames the method of an input",
    );
    for ok in [
        &format!("#[builder(build = X)] {settings}"),
        &format!("#[builder(build = path::X, fallible, crate = my::cb)] {settings}"),
        &format!(
            "#[builder(build = Built<u8>, name = \"ServerBuilder\", entry = \"new\")] {settings}"
        ),
        "#[builder(build = X, entry = \"new\")] struct P<'a> { settings: S, \
         #[builder(input)] id: &'a u8, #[builder(input)] label: Option<String> }",
    ] {
        assert!(!builder_text(ok).contains("compile_error"), "{ok}");
    }
}

#[test]
fn parts_is_refused_as_a_setter_for_a_setting_and_for_an_input() {
    let setting = expand_text("struct S { parts: u8 }")
        .unwrap_err()
        .to_string();
    assert!(setting.contains("`parts`"), "{setting}");
    assert!(setting.contains("setter"), "{setting}");
    let input =
        builder_text("#[builder(build = X)] struct P { settings: S, #[builder(input)] parts: u8 }");
    assert!(input.contains("compile_error"), "{input}");
    assert!(input.contains("isamethodeverybuilderhas"), "{input}");
    let renamed = builder_text(
        "#[builder(build = X)] struct P { settings: S, #[builder(input, setter = \"parts_of\")] parts: u8 }",
    );
    assert!(!renamed.contains("compile_error"), "{renamed}");
    assert!(expand_text("struct S { #[builder(setter = \"parts_of\")] parts: u8 }").is_ok());
}

#[test]
fn only_an_input_that_is_exactly_a_type_parameter_takes_its_type_itself() {
    let text = builder_text(
        "#[builder(build = X, entry = \"new\")] struct P<C, S> { settings: T, #[builder(input)] channel: C, #[builder(input)] stream: Option<S>, #[builder(input)] all: Vec<C>, #[builder(input)] name: String, #[builder(input)] other: core::marker::PhantomData<C>, #[builder(input)] item: C::Item, #[builder(input)] q: <C as Tr>::A, #[builder(input)] p: (C) }",
    );
    assert!(!text.contains("compile_error"), "{text}");
    assert!(text.contains("fnchannel(mutself,value:C)->Self"), "{text}");
    assert!(text.contains("fnp(mutself,value:(C))->Self"), "{text}");
    for (name, into) in [
        ("stream", "Into<S>"),
        ("all", "Into<Vec<C>>"),
        ("name", "Into<String>"),
        ("item", "Into<C::Item>"),
        ("q", "Into<<CasTr>::A>"),
    ] {
        assert!(
            text.contains(&format!(
                "fn{name}(mutself,value:impl::core::convert::{into})"
            )),
            "{name}: {text}"
        );
    }
}

#[test]
fn a_settings_type_refuses_input() {
    let refused = expand_text("struct S { #[builder(input)] id: u8 }")
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("`input` belongs to a parts struct"),
        "{refused}"
    );
}

#[test]
fn a_setter_name_no_method_can_have_is_an_error_not_a_panic() {
    for name in ["self", "max-size", "1x", "Self", "crate", "a b"] {
        let item = format!("struct S {{ #[builder(setter = \"{name}\")] a: u8 }}");
        let refused = expand_text(&item).unwrap_err().to_string();
        assert!(refused.contains("cannot be a setter"), "{name}: {refused}");
        let parts = format!(
            "#[builder(build = X)] struct P {{ settings: S, #[builder(input, setter = \"{name}\")] a: u8 }}"
        );
        let text = builder_text(&parts);
        assert!(text.contains("cannotbeasetter"), "{name}: {text}");
    }
    // A keyword is a method by its raw name.
    assert!(expand_text("struct S { #[builder(setter = \"type\")] a: u8 }").is_ok());
}

#[test]
fn a_setter_takes_a_primitive_scalar_as_itself_and_anything_else_through_into() {
    let mut fields = String::new();
    let mut expected = Vec::new();
    let scalars = [
        "bool", "char", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
        "i128", "isize", "f32", "f64",
    ];
    for (n, ty) in scalars.iter().enumerate() {
        fields.push_str(&format!("s{n}: {ty}, "));
        expected.push((format!("s{n}"), format!("value:{ty})->Self")));
    }
    let others = [
        "Option<u32>",
        "String",
        "Duration",
        "Vec<u8>",
        "Alias",
        "std::primitive::u32",
        "::core::primitive::u32",
        "Wrapper<u8>",
        "&'static u8",
    ];
    for (n, ty) in others.iter().enumerate() {
        fields.push_str(&format!("o{n}: {ty}, "));
        expected.push((
            format!("o{n}"),
            format!("value:impl::core::convert::Into<{}>)", squeezed(ty)),
        ));
    }
    let text = squeezed(
        &expand_text(&format!("struct S {{ {fields} }}"))
            .expect("expands")
            .to_string(),
    );
    for (name, want) in expected {
        assert!(
            text.contains(&format!("fn{name}(self,{want}")),
            "{name}: {want}: {text}"
        );
    }
}

#[test]
fn an_input_setter_takes_a_primitive_scalar_as_itself() {
    let text = builder_text(
        "#[builder(build = X)] struct P { settings: T, #[builder(input)] n: u32, #[builder(input)] f: f32, #[builder(input)] on: bool, #[builder(input)] some: Option<u32>, #[builder(input)] big: Alias, #[builder(input)] text: String }",
    );
    assert!(!text.contains("compile_error"), "{text}");
    for name in ["n:u32", "f:f32", "on:bool", "some:u32"] {
        let (field, ty) = name.split_once(':').unwrap();
        assert!(
            text.contains(&format!("fn{field}(mutself,value:{ty})->Self")),
            "{field}: {text}"
        );
    }
    for (name, into) in [("big", "Into<Alias>"), ("text", "Into<String>")] {
        assert!(
            text.contains(&format!(
                "fn{name}(mutself,value:impl::core::convert::{into})"
            )),
            "{name}: {text}"
        );
    }
}

#[test]
fn setters_on_writes_an_inherent_impl_per_builder() {
    let text = squeezed(
        &expand_text(
            "#[builder(setters_on = XBuilder)] #[builder(setters_on = <'a, C: Tr> YBuilder<'a, C>)] struct S { port: u16, name: String }",
        )
        .expect("expands")
        .to_string(),
    );
    assert!(
        text.contains("implXBuilderwhereSelf:::coqui_builder::builder::Holds<S>{"),
        "{text}"
    );
    assert!(
        text.contains("impl<'a,C:Tr>YBuilder<'a,C>whereSelf:::coqui_builder::builder::Holds<S>{"),
        "{text}"
    );
    assert!(
        text.contains("fnport(self,value:u16)->Self{<SelfasSSetters>::port(self,value)}"),
        "{text}"
    );
    assert!(
        text.contains("fnname(self,value:impl::core::convert::Into<String>)->Self{<SelfasSSetters>::name(self,value)}"),
        "{text}"
    );
}

#[test]
fn setters_on_is_refused_where_it_has_no_meaning() {
    for (item, stem) in [
        (
            "#[builder(setters_on = XBuilder)] struct S<T> { a: T }",
            "generic parameters",
        ),
        (
            "#[builder(setters_on = XBuilder)] enum E { A, B }",
            "named fields",
        ),
        (
            "#[builder(setters_on = XBuilder)] struct S(u8);",
            "named fields",
        ),
        (
            "#[builder(setters_on = XBuilder)] #[map(transparent)] struct S { a: u8 }",
            "named fields",
        ),
        (
            "#[builder(setters_on = )] struct S { a: u8 }",
            "takes the type",
        ),
        (
            "#[builder(setters_on = \"XBuilder\")] struct S { a: u8 }",
            "takes the type",
        ),
        (
            "#[builder(setters_on = <T>)] struct S { a: u8 }",
            "takes the type",
        ),
        ("#[builder(setters_on)] struct S { a: u8 }", "expected `=`"),
    ] {
        let refused = expand_text(item).expect_err(item).to_string();
        assert!(refused.contains(stem), "{item}: {refused}");
    }
}
