use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema};

/// Draft 7 with every subschema inline, the form that grammar converters accept.
pub(crate) fn schema_for<T: JsonSchema>() -> Schema {
    SchemaSettings::draft07()
        .with(|settings| settings.inline_subschemas = true)
        .into_generator()
        .into_root_schema_for::<T>()
}
