//! Bounded data snapshots for PowerShell callback scopes. No object
//! deserialization, property getters, profile statements or command bodies
//! are needed to restore a primitive value, array or dictionary.

use super::CompletionError;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub(super) struct CapturedState {
    module: String,
    definitions: String,
    variables: Value,
}

fn invalid() -> CompletionError {
    CompletionError::Failed("invalid PowerShell captured-state snapshot".into())
}

fn fields(value: &Value, expected: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == expected.len() && expected.iter().all(|name| object.contains_key(*name))
    })
}

fn data(value: &Value, depth: usize, nodes: &mut usize) -> bool {
    *nodes += 1;
    if depth > 8 || *nodes > 1024 || !fields(value, &["kind", "value"]) {
        return false;
    }
    match value["kind"].as_str() {
        Some("Null") => value["value"].is_null(),
        Some("String") => value["value"].is_string(),
        Some("Boolean") => value["value"].is_boolean(),
        Some("Int32") => value["value"]
            .as_i64()
            .is_some_and(|value| i32::try_from(value).is_ok()),
        Some("Int64") => value["value"].as_i64().is_some(),
        Some("Double") => value["value"].as_f64().is_some_and(f64::is_finite),
        Some(
            kind @ ("Array" | "StringArray" | "BooleanArray" | "Int32Array" | "Int64Array"
            | "DoubleArray"),
        ) => value["value"].as_array().is_some_and(|values| {
            values.iter().all(|value| {
                data(value, depth + 1, nodes)
                    && (kind == "Array"
                        || value["kind"].as_str() == kind.strip_suffix("Array")
                        || kind == "StringArray" && value["kind"] == "Null")
            })
        }),
        Some("Map" | "OrderedMap") => value["value"].as_array().is_some_and(|entries| {
            let mut seen = HashSet::new();
            entries.iter().all(|entry| {
                fields(entry, &["key", "value"])
                    && entry["key"]
                        .as_str()
                        .is_some_and(|key| seen.insert(key.to_lowercase()))
                    && data(&entry["value"], depth + 1, nodes)
            })
        }),
        _ => false,
    }
}

impl CapturedState {
    pub(super) fn parse(value: &Value) -> Result<Self, CompletionError> {
        if !fields(value, &["module", "definitions", "variables"]) {
            return Err(invalid());
        }
        let module = value["module"].as_str().ok_or_else(invalid)?;
        let definitions = value["definitions"].as_str().ok_or_else(invalid)?;
        if !module.is_empty() && !super::powershell::is_command_name(module)
            || definitions.contains('\0')
            || value.to_string().len() > 64 * 1024
        {
            return Err(invalid());
        }
        let mut seen = HashSet::new();
        let mut nodes = 0;
        for variable in value["variables"].as_array().ok_or_else(invalid)? {
            let name = variable["name"].as_str().ok_or_else(invalid)?;
            let scope = variable["scope"].as_str().ok_or_else(invalid)?;
            if !fields(variable, &["name", "scope", "value"])
                || !super::powershell::parameter_name(name)
                || name.to_ascii_lowercase().starts_with("notypo")
                || name.to_ascii_lowercase().starts_with("__notypo")
                || !matches!(scope, "script" | "global")
                || !seen.insert((scope, name.to_ascii_lowercase()))
                || !data(&variable["value"], 0, &mut nodes)
            {
                return Err(invalid());
            }
        }
        Ok(Self {
            module: module.into(),
            definitions: definitions.into(),
            variables: value["variables"].clone(),
        })
    }

    fn restore(&self) -> String {
        let quoted = |text: &str| format!("'{}'", text.replace('\'', "''"));
        format!(
            "{DECODE}\n$__notypo_saved = Microsoft.PowerShell.Utility\\ConvertFrom-Json -InputObject {}\n\
            foreach ($__notypo_entry in $__notypo_saved.variables) {{\n\
                $__notypo_value = __notypo_decode $__notypo_entry.value\n\
                Microsoft.PowerShell.Utility\\Set-Variable -Scope $__notypo_entry.scope -Name $__notypo_entry.name -Value $__notypo_value\n\
            }}\n. ([scriptblock]::Create({}))",
            quoted(&serde_json::json!({"variables": self.variables}).to_string()),
            quoted(&self.definitions)
        )
    }

    pub(super) fn definitions(&self) -> Result<String, CompletionError> {
        if !self.module.is_empty() {
            return Err(CompletionError::Unsupported(
                "command scope requires an isolated module".into(),
            ));
        }
        let restored = self.restore();
        if restored.len() > 64 * 1024 || cfg!(windows) && restored.encode_utf16().count() > 32000 {
            return Err(CompletionError::Unsupported(
                "restored PowerShell definitions exceed the snapshot limit".into(),
            ));
        }
        Ok(restored)
    }

    /// The caller authorizes restoring code before this wrapper can run.
    /// A private module keeps helper definitions and captured script variables
    /// in the callback's scope; its body receives the actual binder arguments.
    pub(super) fn wrap_body(&self, body: &str) -> Result<String, CompletionError> {
        let quoted = |text: &str| format!("'{}'", text.replace('\'', "''"));
        let module = if self.module.is_empty() {
            "NotypoCaptured"
        } else {
            &self.module
        };
        let wrapped = format!(
            "if (-not $script:__notypo_captured_module) {{\n\
                $script:__notypo_captured_module = Microsoft.PowerShell.Core\\New-Module -Name {} -ScriptBlock ([scriptblock]::Create({}))\n\
            }}\n\
            $__notypo_callback = $script:__notypo_captured_module.NewBoundScriptBlock([scriptblock]::Create({}))\n\
            & $__notypo_callback @args",
            quoted(module),
            quoted(&self.restore()),
            quoted(body)
        );
        let too_large = if cfg!(windows) {
            wrapped.encode_utf16().count() > 32000
        } else {
            wrapped.len() > 64 * 1024
        };
        if too_large {
            return Err(CompletionError::Unsupported(
                "restored PowerShell callback exceeds the snapshot limit".into(),
            ));
        }
        Ok(wrapped)
    }
}

const DECODE: &str = r#"function __notypo_decode($node) {
    switch ([string]$node.kind) {
        Null { return $null }
        String { return [string]$node.value }
        Boolean { return [bool]$node.value }
        Int32 { return [int]$node.value }
        Int64 { return [long]$node.value }
        Double { return [double]$node.value }
        { $_ -in @('Array','StringArray','BooleanArray','Int32Array','Int64Array','DoubleArray') } {
            $items = @()
            foreach ($item in $node.value) { $items += ,(__notypo_decode $item) }
            switch ([string]$node.kind) {
                StringArray { return ,([string[]]$items) }
                BooleanArray { return ,([bool[]]$items) }
                Int32Array { return ,([int[]]$items) }
                Int64Array { return ,([long[]]$items) }
                DoubleArray { return ,([double[]]$items) }
                default { return ,$items }
            }
        }
        { $_ -in @('Map','OrderedMap') } {
            $items = if ($node.kind -eq 'OrderedMap') { [ordered]@{} } else { @{} }
            foreach ($item in $node.value) { $items[[string]$item.key] = __notypo_decode $item.value }
            return $items
        }
        default { throw 'unsupported captured state' }
    }
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn captured_values_reject_objects_duplicate_names_and_unbounded_nesting() {
        let captured = |value| json!({"module":"Fixture","definitions":"function Get-Helper { $script:team }","variables":[{"name":"team","scope":"script","value":value}]});
        let valid = captured(
            json!({"kind":"Array","value":[{"kind":"String","value":"team one's"},{"kind":"Int32","value":7}]}),
        );
        let state = CapturedState::parse(&valid).unwrap();
        let wrapped = state.wrap_body("param($name) Get-Helper").unwrap();
        assert!(wrapped.contains("NewBoundScriptBlock"));
        assert!(wrapped.contains("team one''''s"));
        for value in [
            json!({"kind":"ScriptBlock","value":"touch marker"}),
            json!({"kind":"Int32","value":2147483648_i64}),
            json!({"kind":"String","value":"fine","type":"System.IO.FileInfo"}),
            json!({"kind":"Map","value":[{"key":"Team","value":{"kind":"Null","value":null}},{"key":"team","value":{"kind":"Null","value":null}}]}),
        ] {
            assert!(CapturedState::parse(&captured(value)).is_err());
        }
        let mut invalid = valid.clone();
        let duplicate = invalid["variables"][0].clone();
        invalid["variables"].as_array_mut().unwrap().push(duplicate);
        assert!(CapturedState::parse(&invalid).is_err());
        let mut value = json!({"kind":"Null","value":null});
        for _ in 0..10 {
            value = json!({"kind":"Array","value":[value]});
        }
        assert!(CapturedState::parse(&captured(value)).is_err());
    }
}
