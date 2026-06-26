use rbx_dom_weak::types::VariantType;
use rbx_reflection::{DataType, PropertyTag, Scriptability};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
struct PropDesc {
    #[serde(rename = "type")]
    ty: String,
    #[serde(rename = "enumType", skip_serializing_if = "Option::is_none")]
    enum_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deprecated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    writable: Option<bool>,
}

#[derive(Serialize)]
struct Catalog {
    enums: BTreeMap<String, Vec<String>>,
    properties: BTreeMap<String, PropDesc>,
    overrides: BTreeMap<String, BTreeMap<String, PropDesc>>,
    classes: BTreeMap<String, Vec<String>>,
    superclasses: BTreeMap<String, Option<String>>,
}

fn variant_type_name(vt: &VariantType) -> &'static str {
    match vt {
        VariantType::Bool => "boolean",
        VariantType::Float32 | VariantType::Float64 => "number",
        VariantType::Int32 | VariantType::Int64 => "number",
        VariantType::String => "string",
        VariantType::Color3 | VariantType::Color3uint8 => "Color3",
        VariantType::Vector3 | VariantType::Vector3int16 => "Vector3",
        VariantType::Vector2 | VariantType::Vector2int16 => "Vector2",
        VariantType::CFrame => "CFrame",
        VariantType::UDim => "UDim",
        VariantType::UDim2 => "UDim2",
        VariantType::NumberRange => "NumberRange",
        VariantType::Rect => "Rect",
        VariantType::BrickColor => "BrickColor",
        VariantType::Ref => "Ref",
        _other => "unknown",
    }
}

fn get_db() -> &'static rbx_reflection::ReflectionDatabase<'static> {
    rbx_reflection_database::get_local()
        .ok()
        .flatten()
        .unwrap_or_else(rbx_reflection_database::get_bundled)
}

fn main() {
    let db = get_db();

    // enums
    let mut enums: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, desc) in &db.enums {
        let mut items: Vec<(u32, String)> = desc
            .items
            .iter()
            .map(|(k, v)| (*v, k.to_string()))
            .collect();
        items.sort_by_key(|(idx, _)| *idx);
        enums.insert(name.to_string(), items.into_iter().map(|(_, n)| n).collect());
    }

    let mut overrides: BTreeMap<String, BTreeMap<String, PropDesc>> = BTreeMap::new();
    let mut classes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut superclasses: BTreeMap<String, Option<String>> = BTreeMap::new();

    for (class_name, class_desc) in &db.classes {
        superclasses.insert(
            class_name.to_string(),
            class_desc.superclass.as_ref().map(|s| s.to_string()),
        );

        let mut prop_names: Vec<String> = Vec::new();
        let mut class_overrides: BTreeMap<String, PropDesc> = BTreeMap::new();

        for (prop_name, prop_desc) in &class_desc.properties {
            let tags = &prop_desc.tags;
            let deprecated = tags.contains(&PropertyTag::Deprecated)
                || tags.contains(&PropertyTag::Hidden)
                || tags.contains(&PropertyTag::NotBrowsable);

            let writable = matches!(
                prop_desc.scriptability,
                Scriptability::ReadWrite | Scriptability::Write
            );

            let (ty, enum_type) = match &prop_desc.data_type {
                DataType::Enum(e) => ("Enum".to_string(), Some(e.to_string())),
                DataType::Value(vt) => (variant_type_name(vt).to_string(), None),
                _ => ("unknown".to_string(), None),
            };

            prop_names.push(prop_name.to_string());
            class_overrides.insert(
                prop_name.to_string(),
                PropDesc {
                    ty,
                    enum_type,
                    category: None,
                    deprecated: if deprecated { Some(true) } else { None },
                    writable: if !writable { Some(false) } else { None },
                },
            );
        }

        prop_names.sort();
        classes.insert(class_name.to_string(), prop_names);
        if !class_overrides.is_empty() {
            overrides.insert(class_name.to_string(), class_overrides);
        }
    }

    let catalog = Catalog {
        enums,
        properties: BTreeMap::new(),
        overrides,
        classes,
        superclasses,
    };

    println!("{}", serde_json::to_string_pretty(&catalog).unwrap());
}
