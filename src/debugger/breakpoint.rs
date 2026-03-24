use std::collections::HashMap;

/// Represents a condition that must be met for a breakpoint to hit
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BreakpointCondition {
    /// Compare a variable (args, step_count) with a value
    Comparison {
        variable: String,
        operator: String,
        value: String,
    },
}

impl BreakpointCondition {
    /// Evaluate the condition against current state
    pub fn evaluate(&self, step_count: usize, args: Option<&str>) -> bool {
        match self {
            Self::Comparison {
                variable,
                operator,
                value,
            } => {
                let current_val = match variable.as_str() {
                    "step_count" | "steps" => step_count.to_string(),
                    "args" | "arguments" => args.unwrap_or("").to_string(),
                    _ => return false,
                };

                match operator.as_str() {
                    "==" => current_val == *value,
                    "!=" => current_val != *value,
                    ">" => {
                        if let (Ok(curr), Ok(val)) = (current_val.parse::<i64>(), value.parse::<i64>())
                        {
                            curr > val
                        } else {
                            current_val > *value
                        }
                    }
                    "<" => {
                        if let (Ok(curr), Ok(val)) = (current_val.parse::<i64>(), value.parse::<i64>())
                        {
                            curr < val
                        } else {
                            current_val < *value
                        }
                    }
                    ">=" => {
                        if let (Ok(curr), Ok(val)) = (current_val.parse::<i64>(), value.parse::<i64>())
                        {
                            curr >= val
                        } else {
                            current_val >= *value
                        }
                    }
                    "<=" => {
                        if let (Ok(curr), Ok(val)) = (current_val.parse::<i64>(), value.parse::<i64>())
                        {
                            curr <= val
                        } else {
                            current_val <= *value
                        }
                    }
                    _ => false,
                }
            }
        }
    }
}

/// Information about a set breakpoint
#[derive(Debug, Clone)]
pub struct Breakpoint {
    pub function: String,
    pub condition: Option<BreakpointCondition>,
}

/// Manages breakpoints during debugging
pub struct BreakpointManager {
    breakpoints: HashMap<String, Breakpoint>,
}

impl BreakpointManager {
    /// Create a new breakpoint manager
    pub fn new() -> Self {
        Self {
            breakpoints: HashMap::new(),
        }
    }

    /// Add a breakpoint at a function name with an optional condition
    pub fn add(&mut self, function: &str, condition: Option<&str>) -> crate::Result<()> {
        let condition = if let Some(c) = condition {
            Some(self.parse_condition(c)?)
        } else {
            None
        };

        self.breakpoints.insert(
            function.to_string(),
            Breakpoint {
                function: function.to_string(),
                condition,
            },
        );
        Ok(())
    }

    /// Remove a breakpoint
    pub fn remove(&mut self, function: &str) -> bool {
        self.breakpoints.remove(function).is_some()
    }

    /// Get a breakpoint by function name
    pub fn get_breakpoint(&self, function: &str) -> Option<&Breakpoint> {
        self.breakpoints.get(function)
    }

    /// Check if execution should break at this function given the current state
    pub fn should_break(&self, function: &str, step_count: usize, args: Option<&str>) -> bool {
        if let Some(bp) = self.breakpoints.get(function) {
            if let Some(condition) = &bp.condition {
                condition.evaluate(step_count, args)
            } else {
                true
            }
        } else {
            false
        }
    }

    /// List all breakpoints
    pub fn list(&self) -> Vec<String> {
        self.breakpoints.keys().cloned().collect()
    }

    /// Get detailed list of breakpoints
    pub fn list_detailed(&self) -> Vec<Breakpoint> {
        self.breakpoints.values().cloned().collect()
    }

    /// Clear all breakpoints
    pub fn clear(&mut self) {
        self.breakpoints.clear();
    }

    /// Check if there are any breakpoints set
    pub fn is_empty(&self) -> bool {
        self.breakpoints.is_empty()
    }

    /// Get count of breakpoints
    pub fn count(&self) -> usize {
        self.breakpoints.len()
    }

    /// Parse a condition string into a BreakpointCondition object
    pub fn parse_condition(&self, s: &str) -> crate::Result<BreakpointCondition> {
        let s = s.trim();
        if let Some((op, pos)) = find_operator(s) {
            let variable = s[..pos].trim().to_string();
            let mut value = s[pos + op.len()..].trim().to_string();

            // Strip quotes if present
            if (value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\''))
            {
                value = value[1..value.len() - 1].to_string();
            }

            Ok(BreakpointCondition::Comparison {
                variable,
                operator: op.to_string(),
                value,
            })
        } else {
            use crate::DebuggerError;
            Err(DebuggerError::BreakpointError(format!(
                "Invalid breakpoint condition format: '{}'. Expected 'variable op value'",
                s
            ))
            .into())
        }
    }
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
        let _ = manager.add("transfer", None);
        assert!(manager.should_break("transfer", 0, None));
        assert!(!manager.should_break("mint", 0, None));
    }

    #[test]
    fn test_conditional_breakpoint() {
        let mut manager = BreakpointManager::new();
        let _ = manager.add("transfer", Some("step_count > 5"));

        assert!(!manager.should_break("transfer", 3, None));
        assert!(manager.should_break("transfer", 6, None));

        let _ = manager.add("mint", Some("args == 'high'"));
        assert!(!manager.should_break("mint", 0, Some("low")));
        assert!(manager.should_break("mint", 0, Some("high")));
    }

    #[test]
    fn test_remove_breakpoint() {
        let mut manager = BreakpointManager::new();
        let _ = manager.add("transfer", None);
        assert!(manager.remove("transfer"));
        assert!(!manager.should_break("transfer", 0, None));
    }
}
