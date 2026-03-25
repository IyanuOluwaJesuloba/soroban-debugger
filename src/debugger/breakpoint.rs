use crate::{DebuggerError, Result};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operator {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    Storage {
        key: String,
        operator: Operator,
        value: String,
    },
    Argument {
        name: String,
        operator: Operator,
        value: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitCondition {
    Eq(u64),
    Ge(u64),
    Gt(u64),
    Le(u64),
    Lt(u64),
    MultipleOf(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakpointSpec {
    pub id: String,
    pub function: String,
    pub condition: Option<Condition>,
    pub hit_condition: Option<HitCondition>,
    pub log_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakpointSummary {
    pub id: String,
    pub function: String,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakpointHit {
    pub should_pause: bool,
    pub matched_ids: Vec<String>,
    pub log_messages: Vec<String>,
}

#[derive(Debug, Clone)]
struct ManagedBreakpoint {
    spec: BreakpointSpec,
    hit_count: u64,
}

/// Manages breakpoints during debugging.
pub struct BreakpointManager {
    breakpoints: HashMap<String, ManagedBreakpoint>,
}

impl BreakpointManager {
    /// Create a new breakpoint manager.
    pub fn new() -> Self {
        Self {
            breakpoints: HashMap::new(),
        }
    }

    /// Add a plain function breakpoint.
    pub fn add_simple(&mut self, function: &str) {
        self.add(BreakpointSpec {
            id: function.to_string(),
            function: function.to_string(),
            condition: None,
            hit_condition: None,
            log_message: None,
        });
    }

    /// Add or replace a breakpoint.
    pub fn add(&mut self, spec: BreakpointSpec) {
        self.breakpoints.insert(
            spec.id.clone(),
            ManagedBreakpoint {
                spec,
                hit_count: 0,
            },
        );
    }

    /// Remove a breakpoint by id.
    pub fn remove(&mut self, breakpoint_id: &str) -> bool {
        self.breakpoints.remove(breakpoint_id).is_some()
    }

    /// Remove the default plain-function breakpoint used by the TUI/client.
    pub fn remove_function(&mut self, function: &str) -> bool {
        self.remove(function)
    }

    /// Check if execution should consider a breakpoint at this function.
    pub fn should_break(&self, function: &str) -> bool {
        self.breakpoints
            .values()
            .any(|breakpoint| breakpoint.spec.function == function)
    }

    /// Evaluate function-entry breakpoints against the current storage and args.
    pub fn on_hit(
        &mut self,
        function: &str,
        storage: &HashMap<String, String>,
        args: Option<&str>,
    ) -> Result<Option<BreakpointHit>> {
        let mut should_pause = false;
        let mut matched_ids = Vec::new();
        let mut log_messages = Vec::new();

        for breakpoint in self
            .breakpoints
            .values_mut()
            .filter(|breakpoint| breakpoint.spec.function == function)
        {
            breakpoint.hit_count = breakpoint.hit_count.saturating_add(1);

            if !Self::condition_matches(breakpoint.spec.condition.as_ref(), storage, args)? {
                continue;
            }

            if !Self::hit_condition_matches(
                breakpoint.spec.hit_condition.as_ref(),
                breakpoint.hit_count,
            ) {
                continue;
            }

            matched_ids.push(breakpoint.spec.id.clone());
            if let Some(template) = breakpoint.spec.log_message.as_deref() {
                log_messages.push(Self::render_log_message(template, function, storage, args));
                continue;
            }

            should_pause = true;
        }

        if matched_ids.is_empty() {
            return Ok(None);
        }

        Ok(Some(BreakpointHit {
            should_pause,
            matched_ids,
            log_messages,
        }))
    }

    /// List all breakpoint ids.
    pub fn list(&self) -> Vec<String> {
        self.breakpoints.keys().cloned().collect()
    }

    /// List structured breakpoint details.
    pub fn list_detailed(&self) -> Vec<BreakpointSummary> {
        self.breakpoints
            .values()
            .map(|breakpoint| BreakpointSummary {
                id: breakpoint.spec.id.clone(),
                function: breakpoint.spec.function.clone(),
                condition: breakpoint.spec.condition.as_ref().map(Self::format_condition),
                hit_condition: breakpoint
                    .spec
                    .hit_condition
                    .as_ref()
                    .map(Self::format_hit_condition),
                log_message: breakpoint.spec.log_message.clone(),
            })
            .collect()
    }

    /// Clear all breakpoints.
    pub fn clear(&mut self) {
        self.breakpoints.clear();
    }

    /// Check if there are any breakpoints set.
    pub fn is_empty(&self) -> bool {
        self.breakpoints.is_empty()
    }

    /// Get count of breakpoints.
    pub fn count(&self) -> usize {
        self.breakpoints.len()
    }

    /// Parse a condition string into a Condition object.
    pub fn parse_condition(s: &str) -> Result<Condition> {
        let trimmed = s.trim();
        let (operator, pos) = find_operator(trimmed).ok_or_else(|| {
            DebuggerError::BreakpointError(format!(
                "Unsupported breakpoint condition '{trimmed}'. Expected formats like \
                 'storage[key] == value' or 'arg_name >= 42'."
            ))
        })?;

        let left = trimmed[..pos].trim();
        let right = trimmed[pos + operator.len()..].trim();
        if left.is_empty() || right.is_empty() {
            return Err(DebuggerError::BreakpointError(format!(
                "Invalid breakpoint condition '{trimmed}'. Both sides of the operator are required."
            ))
            .into());
        }

        let operator = parse_operator(operator);
        if let Some(key) = parse_storage_key(left) {
            return Ok(Condition::Storage {
                key,
                operator,
                value: strip_wrapping_quotes(right),
            });
        }

        if is_identifier(left) {
            return Ok(Condition::Argument {
                name: left.to_string(),
                operator,
                value: strip_wrapping_quotes(right),
            });
        }

        Err(DebuggerError::BreakpointError(format!(
            "Unsupported breakpoint condition target '{left}' in '{trimmed}'. Use \
             'storage[key]' or a named argument."
        ))
        .into())
    }

    /// Parse a DAP hit-count expression.
    pub fn parse_hit_condition(s: &str) -> Result<HitCondition> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(DebuggerError::BreakpointError(
                "Hit condition cannot be empty".to_string(),
            )
            .into());
        }

        if let Some(rest) = trimmed.strip_prefix('%') {
            let value = parse_positive_u64(rest.trim(), trimmed)?;
            return Ok(HitCondition::MultipleOf(value));
        }

        for (prefix, builder) in [
            (">=", HitCondition::Ge as fn(u64) -> HitCondition),
            ("<=", HitCondition::Le),
            ("==", HitCondition::Eq),
            (">", HitCondition::Gt),
            ("<", HitCondition::Lt),
            ("=", HitCondition::Eq),
        ] {
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                let value = parse_positive_u64(rest.trim(), trimmed)?;
                return Ok(builder(value));
            }
        }

        Ok(HitCondition::Eq(parse_positive_u64(trimmed, trimmed)?))
    }

    fn condition_matches(
        condition: Option<&Condition>,
        storage: &HashMap<String, String>,
        args: Option<&str>,
    ) -> Result<bool> {
        let Some(condition) = condition else {
            return Ok(true);
        };

        let (actual, expected, operator) = match condition {
            Condition::Storage {
                key,
                operator,
                value,
            } => (storage.get(key).cloned(), Some(value.clone()), operator),
            Condition::Argument {
                name,
                operator,
                value,
            } => (
                extract_argument_value(args, name),
                Some(value.clone()),
                operator,
            ),
        };

        let Some(actual) = actual else {
            return Ok(false);
        };
        let expected = expected.unwrap_or_default();
        Ok(compare_values(&actual, &expected, operator))
    }

    fn hit_condition_matches(hit_condition: Option<&HitCondition>, hit_count: u64) -> bool {
        match hit_condition {
            None => true,
            Some(HitCondition::Eq(expected)) => hit_count == *expected,
            Some(HitCondition::Ge(expected)) => hit_count >= *expected,
            Some(HitCondition::Gt(expected)) => hit_count > *expected,
            Some(HitCondition::Le(expected)) => hit_count <= *expected,
            Some(HitCondition::Lt(expected)) => hit_count < *expected,
            Some(HitCondition::MultipleOf(expected)) => hit_count % *expected == 0,
        }
    }

    fn render_log_message(
        template: &str,
        function: &str,
        storage: &HashMap<String, String>,
        args: Option<&str>,
    ) -> String {
        let mut rendered = String::new();
        let mut remaining = template;

        while let Some(start) = remaining.find('{') {
            rendered.push_str(&remaining[..start]);
            let placeholder_start = start + 1;
            if let Some(end_rel) = remaining[placeholder_start..].find('}') {
                let end = placeholder_start + end_rel;
                let expression = remaining[placeholder_start..end].trim();
                rendered.push_str(
                    &Self::resolve_log_expression(expression, function, storage, args)
                        .unwrap_or_else(|| format!("{{{expression}}}")),
                );
                remaining = &remaining[end + 1..];
            } else {
                rendered.push_str(&remaining[start..]);
                return rendered;
            }
        }

        rendered.push_str(remaining);
        rendered
    }

    fn resolve_log_expression(
        expression: &str,
        function: &str,
        storage: &HashMap<String, String>,
        args: Option<&str>,
    ) -> Option<String> {
        if expression.eq_ignore_ascii_case("function") {
            return Some(function.to_string());
        }
        if expression.eq_ignore_ascii_case("args") {
            return Some(args.unwrap_or_default().to_string());
        }
        if let Some(key) = parse_storage_key(expression) {
            return storage.get(&key).cloned();
        }
        extract_argument_value(args, expression)
    }

    fn format_condition(condition: &Condition) -> String {
        match condition {
            Condition::Storage {
                key,
                operator,
                value,
            } => format!("storage[{key}] {} {value}", format_operator(operator)),
            Condition::Argument {
                name,
                operator,
                value,
            } => format!("{name} {} {value}", format_operator(operator)),
        }
    }

    fn format_hit_condition(hit_condition: &HitCondition) -> String {
        match hit_condition {
            HitCondition::Eq(value) => value.to_string(),
            HitCondition::Ge(value) => format!(">= {value}"),
            HitCondition::Gt(value) => format!("> {value}"),
            HitCondition::Le(value) => format!("<= {value}"),
            HitCondition::Lt(value) => format!("< {value}"),
            HitCondition::MultipleOf(value) => format!("% {value}"),
        }
    }
}

fn parse_positive_u64(raw: &str, original: &str) -> Result<u64> {
    let value = raw.parse::<u64>().map_err(|_| {
        DebuggerError::BreakpointError(format!(
            "Invalid hit condition '{original}'. Expected a positive integer."
        ))
    })?;
    if value == 0 {
        return Err(DebuggerError::BreakpointError(format!(
            "Invalid hit condition '{original}'. Zero is not allowed."
        ))
        .into());
    }
    Ok(value)
}

fn find_operator(s: &str) -> Option<(&'static str, usize)> {
    let ops = [">=", "<=", "==", "!=", ">", "<"];
    for op in ops {
        if let Some(pos) = s.find(op) {
            return Some((op, pos));
        }
    }
    None
}

fn parse_operator(s: &str) -> Operator {
    match s {
        "==" => Operator::Eq,
        "!=" => Operator::Ne,
        ">" => Operator::Gt,
        ">=" => Operator::Ge,
        "<" => Operator::Lt,
        "<=" => Operator::Le,
        _ => Operator::Eq,
    }
}

fn format_operator(operator: &Operator) -> &'static str {
    match operator {
        Operator::Eq => "==",
        Operator::Ne => "!=",
        Operator::Gt => ">",
        Operator::Ge => ">=",
        Operator::Lt => "<",
        Operator::Le => "<=",
    }
}

fn parse_storage_key(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if !trimmed.starts_with("storage[") || !trimmed.ends_with(']') {
        return None;
    }
    let inner = &trimmed["storage[".len()..trimmed.len() - 1];
    let key = strip_wrapping_quotes(inner.trim());
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

fn strip_wrapping_quotes(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.len() >= 2 {
        let starts_with_quote = trimmed.starts_with('"') && trimmed.ends_with('"');
        let starts_with_single = trimmed.starts_with('\'') && trimmed.ends_with('\'');
        if starts_with_quote || starts_with_single {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
    }
    trimmed.to_string()
}

fn is_identifier(input: &str) -> bool {
    let mut chars = input.chars();
    match chars.next() {
        Some(first) if first == '_' || first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|char| char == '_' || char.is_ascii_alphanumeric())
}

fn extract_argument_value(args: Option<&str>, name: &str) -> Option<String> {
    let args = args?;
    let parsed: Value = serde_json::from_str(args).ok()?;

    match parsed {
        Value::Object(map) => map.get(name).map(json_value_to_string),
        Value::Array(items) => items.into_iter().find_map(|item| match item {
            Value::Object(map) => map.get(name).map(json_value_to_string),
            _ => None,
        }),
        _ => None,
    }
}

fn json_value_to_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

fn compare_values(actual: &str, expected: &str, operator: &Operator) -> bool {
    if let (Ok(actual_number), Ok(expected_number)) =
        (actual.parse::<f64>(), expected.parse::<f64>())
    {
        return match operator {
            Operator::Eq => actual_number == expected_number,
            Operator::Ne => actual_number != expected_number,
            Operator::Gt => actual_number > expected_number,
            Operator::Ge => actual_number >= expected_number,
            Operator::Lt => actual_number < expected_number,
            Operator::Le => actual_number <= expected_number,
        };
    }

    match operator {
        Operator::Eq => actual == expected,
        Operator::Ne => actual != expected,
        Operator::Gt => actual > expected,
        Operator::Ge => actual >= expected,
        Operator::Lt => actual < expected,
        Operator::Le => actual <= expected,
    }
}

impl Default for BreakpointManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_breakpoint() {
        let mut manager = BreakpointManager::new();
        manager.add_simple("transfer");
        assert!(manager.should_break("transfer"));
        assert!(!manager.should_break("mint"));
    }

    #[test]
    fn test_conditional_breakpoint() {
        let mut manager = BreakpointManager::new();
        manager.add_simple("transfer");
        assert!(manager.remove("transfer"));
        assert!(!manager.should_break("transfer"));
    }

    #[test]
    fn test_remove_breakpoint() {
        let mut manager = BreakpointManager::new();
        manager.add_simple("transfer");
        manager.add_simple("mint");
        let list = manager.list();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&"transfer".to_string()));
        assert!(list.contains(&"mint".to_string()));
    }

    #[test]
    fn test_parse_storage_condition() {
        let cond = BreakpointManager::parse_condition("storage[balance] > 100").unwrap();
        match cond {
            Condition::Storage {
                key,
                operator,
                value,
            } => {
                assert_eq!(key, "balance");
                assert_eq!(operator, Operator::Gt);
                assert_eq!(value, "100");
            }
            _ => panic!("Expected storage condition"),
        }
    }

    #[test]
    fn test_parse_argument_condition() {
        let cond = BreakpointManager::parse_condition("amount >= 500").unwrap();
        match cond {
            Condition::Argument {
                name,
                operator,
                value,
            } => {
                assert_eq!(name, "amount");
                assert_eq!(operator, Operator::Ge);
                assert_eq!(value, "500");
            }
            _ => panic!("Expected argument condition"),
        }
    }

    #[test]
    fn test_parse_hit_condition_variants() {
        assert_eq!(
            BreakpointManager::parse_hit_condition("5").unwrap(),
            HitCondition::Eq(5)
        );
        assert_eq!(
            BreakpointManager::parse_hit_condition(">= 3").unwrap(),
            HitCondition::Ge(3)
        );
        assert_eq!(
            BreakpointManager::parse_hit_condition("% 2").unwrap(),
            HitCondition::MultipleOf(2)
        );
    }

    #[test]
    fn test_evaluate_storage_condition() {
        let mut manager = BreakpointManager::new();
        let cond = BreakpointManager::parse_condition("storage[user1] == Bob").unwrap();
        manager.add(BreakpointSpec {
            id: "bp-1".to_string(),
            function: "test_func".to_string(),
            condition: Some(cond),
            hit_condition: None,
            log_message: None,
        });

        let mut storage = HashMap::new();
        storage.insert("user1".to_string(), "Bob".to_string());

        let hit = manager.on_hit("test_func", &storage, None).unwrap().unwrap();
        assert!(hit.should_pause);

        storage.insert("user1".to_string(), "Alice".to_string());
        assert!(manager.on_hit("test_func", &storage, None).unwrap().is_none());
    }

    #[test]
    fn test_evaluate_numeric_condition() {
        let mut manager = BreakpointManager::new();
        let cond = BreakpointManager::parse_condition("amount > 1000").unwrap();
        manager.add(BreakpointSpec {
            id: "bp-2".to_string(),
            function: "test_func".to_string(),
            condition: Some(cond),
            hit_condition: None,
            log_message: None,
        });

        let storage = HashMap::new();

        assert!(manager
            .on_hit("test_func", &storage, Some("{\"amount\": 1500}"))
            .unwrap()
            .unwrap()
            .should_pause);
        assert!(manager
            .on_hit("test_func", &storage, Some("{\"amount\": 500}"))
            .unwrap()
            .is_none());
        assert!(manager
            .on_hit("test_func", &storage, Some("{\"amount\": 1000}"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_hit_condition_delays_pause() {
        let mut manager = BreakpointManager::new();
        manager.add(BreakpointSpec {
            id: "bp-3".to_string(),
            function: "test_func".to_string(),
            condition: None,
            hit_condition: Some(HitCondition::Ge(3)),
            log_message: None,
        });

        let storage = HashMap::new();
        assert!(manager.on_hit("test_func", &storage, None).unwrap().is_none());
        assert!(manager.on_hit("test_func", &storage, None).unwrap().is_none());
        assert!(manager
            .on_hit("test_func", &storage, None)
            .unwrap()
            .unwrap()
            .should_pause);
    }

    #[test]
    fn test_logpoint_renders_values_without_pausing() {
        let mut manager = BreakpointManager::new();
        manager.add(BreakpointSpec {
            id: "bp-4".to_string(),
            function: "transfer".to_string(),
            condition: None,
            hit_condition: None,
            log_message: Some(
                "transfer amount={amount} balance={storage[balance]} fn={function}".to_string(),
            ),
        });

        let mut storage = HashMap::new();
        storage.insert("balance".to_string(), "42".to_string());

        let hit = manager
            .on_hit("transfer", &storage, Some("{\"amount\": 9}"))
            .unwrap()
            .unwrap();

        assert!(!hit.should_pause);
        assert_eq!(
            hit.log_messages,
            vec!["transfer amount=9 balance=42 fn=transfer".to_string()]
        );
    }

    #[test]
    fn test_invalid_condition_syntax_reports_helpful_message() {
        let err = BreakpointManager::parse_condition("amount").unwrap_err();
        assert!(err.to_string().contains("Unsupported breakpoint condition"));
    }

    #[test]
    fn test_invalid_hit_condition_reports_helpful_message() {
        let err = BreakpointManager::parse_hit_condition("% 0").unwrap_err();
        assert!(err.to_string().contains("Zero is not allowed"));
    }
}
