use serde::{Deserialize, Serialize};
use std::fmt;

/// Current protocol version implemented by this backend.
pub const PROTOCOL_VERSION: u32 = 1;
/// Minimum protocol version this backend can communicate with.
pub const PROTOCOL_MIN_VERSION: u32 = 1;
/// Maximum protocol version this backend can communicate with.
pub const PROTOCOL_MAX_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolNegotiationError {
    InvalidClientRange {
        min: u32,
        max: u32,
    },
    NoOverlap {
        client_min: u32,
        client_max: u32,
        server_min: u32,
        server_max: u32,
    },
}

impl fmt::Display for ProtocolNegotiationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidClientRange { min, max } => {
                write!(
                    f,
                    "Invalid client protocol range (min={} > max={})",
                    min, max
                )
            }
            Self::NoOverlap {
                client_min,
                client_max,
                server_min,
                server_max,
            } => write!(
                f,
                "Protocol mismatch: client supports [{}..={}], server supports [{}..={}]",
                client_min, client_max, server_min, server_max
            ),
        }
    }
}

impl std::error::Error for ProtocolNegotiationError {}

pub fn negotiate_protocol_version(
    client_min: u32,
    client_max: u32,
) -> Result<u32, ProtocolNegotiationError> {
    if client_min > client_max {
        return Err(ProtocolNegotiationError::InvalidClientRange {
            min: client_min,
            max: client_max,
        });
    }

    let negotiated_min = client_min.max(PROTOCOL_MIN_VERSION);
    let negotiated_max = client_max.min(PROTOCOL_MAX_VERSION);
    if negotiated_min > negotiated_max {
        return Err(ProtocolNegotiationError::NoOverlap {
            client_min,
            client_max,
            server_min: PROTOCOL_MIN_VERSION,
            server_max: PROTOCOL_MAX_VERSION,
        });
    }

    Ok(negotiated_max)
}

/// Structured event category used by dynamic security analysis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum DynamicTraceEventKind {
    #[default]
    Diagnostic,
    FunctionCall,
    StorageRead,
    StorageWrite,
    Authorization,
    CrossContractCall,
    CrossContractReturn,
}

/// Rich dynamic trace entry produced by the runtime and consumed by analyzers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DynamicTraceEvent {
    pub sequence: usize,
    pub kind: DynamicTraceEventKind,
    pub message: String,
    pub caller: Option<String>,
    pub function: Option<String>,
    pub call_depth: Option<usize>,
    pub storage_key: Option<String>,
    pub storage_value: Option<String>,
    /// Call-frame depth at the time this event was emitted (0 = top-level).
    /// Used by reentrancy analysis to correlate writes with the frame that
    /// issued the cross-contract call.
    #[serde(default)]
    pub call_depth: u32,
}

/// Source location information (file, line, column)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceLocation {
    /// Source file path (relative or absolute)
    pub file: String,
    /// 1-based line number
    pub line: u32,
    /// 0-based column (optional)
    pub column: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointCapabilities {
    pub conditional_breakpoints: bool,
    pub hit_conditional_breakpoints: bool,
    pub log_points: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakpointDescriptor {
    pub id: String,
    pub function: String,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
}

/// Wire protocol messages for remote debugging
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DebugRequest {
    /// Protocol handshake / version negotiation.
    Handshake {
        client_name: String,
        client_version: String,
        protocol_min: u32,
        protocol_max: u32,
    },

    /// Authenticate with the server
    Authenticate { token: String },

    /// Load a contract
    LoadContract { contract_path: String },

    /// Execute a function
    Execute {
        function: String,
        args: Option<String>,
    },

    /// Step execution (instruction-level)
    Step,
    /// Step into next inline/instruction
    StepIn,

    /// Step over current function
    Next,

    /// Step out of current function
    StepOut,

    /// Step over to next source line in the same frame
    StepOverLine,

    /// Continue execution
    Continue,

    /// Inspect current state
    Inspect,

    /// Get storage state
    GetStorage,

    /// Get call stack
    GetStack,

    /// Get budget information
    GetBudget,

    /// Set a breakpoint
    SetBreakpoint {
        id: String,
        function: String,
        condition: Option<String>,
        hit_condition: Option<String>,
        log_message: Option<String>,
    },

    /// Clear a breakpoint
    ClearBreakpoint { id: String },

    /// List all breakpoints
    ListBreakpoints,

    /// Get backend debugging capabilities
    GetCapabilities,

    /// Set initial storage
    SetStorage { storage_json: String },

    /// Load network snapshot
    LoadSnapshot { snapshot_path: String },

    /// Evaluate an expression in the current debug context
    Evaluate {
        expression: String,
        frame_id: Option<u64>,
    },

    /// Ping to check connection
    Ping,

    /// Disconnect
    Disconnect,
}

/// Response messages from the server
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DebugResponse {
    /// Handshake successful. Both sides have at least one compatible protocol version.
    HandshakeAck {
        server_name: String,
        server_version: String,
        protocol_min: u32,
        protocol_max: u32,
        selected_version: u32,
    },

    /// Handshake failed due to protocol mismatch.
    IncompatibleProtocol {
        message: String,
        server_name: String,
        server_version: String,
        protocol_min: u32,
        protocol_max: u32,
    },

    /// Authentication result
    Authenticated { success: bool, message: String },

    /// Contract loaded
    ContractLoaded { size: usize },

    /// Execution result
    ExecutionResult {
        success: bool,
        output: String,
        error: Option<String>,
        paused: bool,
        completed: bool,
        source_location: Option<SourceLocation>,
    },

    /// Step result
    StepResult {
        paused: bool,
        current_function: Option<String>,
        step_count: u64,
        source_location: Option<SourceLocation>,
    },

    /// Source-level step-over result
    StepOverLineResult {
        paused: bool,
        file: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },

    /// Continue result
    ContinueResult {
        completed: bool,
        output: Option<String>,
        error: Option<String>,
        paused: bool,
        source_location: Option<SourceLocation>,
    },

    /// Inspection result
    InspectionResult {
        function: Option<String>,
        args: Option<String>,
        step_count: u64,
        paused: bool,
        call_stack: Vec<String>,
        source_location: Option<SourceLocation>,
    },

    /// Storage state
    StorageState { storage_json: String },

    /// Call stack
    CallStack { stack: Vec<String> },

    /// Budget information
    BudgetInfo {
        cpu_instructions: u64,
        memory_bytes: u64,
    },

    /// Breakpoint set
    BreakpointSet { id: String, function: String },

    /// Breakpoint cleared
    BreakpointCleared { id: String },

    /// List of breakpoints
    BreakpointsList { breakpoints: Vec<BreakpointDescriptor> },

    /// Backend capabilities
    Capabilities {
        breakpoints: BreakpointCapabilities,
    },

    /// Snapshot loaded
    SnapshotLoaded { summary: String },

    /// Error response
    Error { message: String },

    /// Evaluation result
    EvaluateResult {
        result: String,
        result_type: Option<String>,
        variables_reference: u64,
    },

    /// Pong response
    Pong,

    /// Disconnected
    Disconnected,
}

/// Message wrapper for the protocol
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugMessage {
    /// Correlation id used to match a response to the originating request.
    pub id: u64,
    pub request: Option<DebugRequest>,
    pub response: Option<DebugResponse>,
}

impl DebugMessage {
    pub fn request(id: u64, request: DebugRequest) -> Self {
        Self {
            id,
            request: Some(request),
            response: None,
        }
    }

    pub fn response(id: u64, response: DebugResponse) -> Self {
        Self {
            id,
            request: None,
            response: Some(response),
        }
    }

    pub fn is_response_for(&self, expected_id: u64) -> bool {
        self.id == expected_id && self.response.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiate_protocol_version_accepts_exact_match() {
        let v = negotiate_protocol_version(PROTOCOL_MIN_VERSION, PROTOCOL_MAX_VERSION).unwrap();
        assert_eq!(v, PROTOCOL_VERSION);
    }

    #[test]
    fn negotiate_protocol_version_selects_highest_common_version() {
        let v = negotiate_protocol_version(0, 999).unwrap();
        assert_eq!(v, PROTOCOL_MAX_VERSION);
    }

    #[test]
    fn negotiate_protocol_version_rejects_older_client() {
        let err = negotiate_protocol_version(0, PROTOCOL_MIN_VERSION - 1).unwrap_err();
        assert!(matches!(err, ProtocolNegotiationError::NoOverlap { .. }));
        assert!(err.to_string().contains("Protocol mismatch"));
    }

    #[test]
    fn negotiate_protocol_version_rejects_newer_client() {
        let err = negotiate_protocol_version(PROTOCOL_MAX_VERSION + 1, PROTOCOL_MAX_VERSION + 2)
            .unwrap_err();
        assert!(matches!(err, ProtocolNegotiationError::NoOverlap { .. }));
        assert!(err.to_string().contains("Protocol mismatch"));
    }

    #[test]
    fn negotiate_protocol_version_rejects_malformed_range() {
        let err = negotiate_protocol_version(2, 1).unwrap_err();
        assert!(matches!(
            err,
            ProtocolNegotiationError::InvalidClientRange { .. }
        ));
        assert!(err.to_string().contains("Invalid client protocol range"));
    }
}
