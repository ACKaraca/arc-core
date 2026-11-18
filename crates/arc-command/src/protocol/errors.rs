//! The error model of the protocol (CORE-CMD-027): one mapping from [`arc_base::Error`] to the
//! JSON-RPC error object.

use arc_base::{Category, Error};
use serde_json::{Map, Value, json};

/// JSON-RPC: the message is not JSON.
pub const PARSE_ERROR: i64 = -32700;
/// JSON-RPC: the message is not a valid request.
pub const INVALID_REQUEST: i64 = -32600;
/// JSON-RPC: the method does not exist.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC: the method's parameters are malformed.
pub const INVALID_PARAMS: i64 = -32602;
/// JSON-RPC: a fault while handling the message.
pub const INTERNAL_ERROR: i64 = -32603;
/// Unknown command or method in this product.
pub const UNKNOWN: i64 = -32001;
/// Parameters violate the schema.
pub const SCHEMA: i64 = -32002;
/// Command disabled.
pub const DISABLED: i64 = -32003;
/// Permission class not granted.
pub const PERMISSION: i64 = -32004;
/// Document or object not found.
pub const NOT_FOUND: i64 = -32005;
/// Revision mismatch or conflict.
pub const CONFLICT: i64 = -32006;
/// Cancelled.
pub const CANCELLED: i64 = -32007;
/// Limit exceeded.
pub const LIMIT: i64 = -32008;
/// Path outside the read or write roots.
pub const OUTSIDE_ROOTS: i64 = -32009;
/// Command failed and was rolled back.
pub const FAILED: i64 = -32010;
/// Unsupported protocol version.
pub const VERSION: i64 = -32011;
/// Internal fault; the document was restored.
pub const FAULT: i64 = -32012;

/// The protocol code of `error`: by its code where the table names one, else by its category.
#[must_use]
pub fn rpc_code(error: &Error) -> i64 {
    match error.code() {
        "cmd.protocol.parse" => PARSE_ERROR,
        "cmd.protocol.invalid_request"
        | "cmd.protocol.not_initialized"
        | "cmd.protocol.already_initialized" => INVALID_REQUEST,
        "cmd.protocol.method_unknown" => METHOD_NOT_FOUND,
        "cmd.protocol.invalid_params" => INVALID_PARAMS,
        "cmd.protocol.internal" => INTERNAL_ERROR,
        "cmd.unknown" | "cmd.not_exposed" => UNKNOWN,
        "cmd.disabled" => DISABLED,
        "cmd.path.outside_roots" => OUTSIDE_ROOTS,
        "cmd.protocol.version_unsupported" => VERSION,
        _ => match error.category() {
            Category::InvalidInput => SCHEMA,
            Category::NotFound => NOT_FOUND,
            Category::Conflict => CONFLICT,
            Category::PermissionDenied => PERMISSION,
            Category::Cancelled => CANCELLED,
            Category::LimitExceeded => LIMIT,
            Category::Unsupported => UNKNOWN,
            Category::Io | Category::CorruptData | Category::FailedOperation => FAILED,
            Category::Internal => FAULT,
        },
    }
}

fn json_of(value: &arc_base::error::Value) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// The `data` member of `error`: `code`, `category`, `messageId`, `arguments` and `details`.
/// The detail `panic` is left out (it goes to the log), the detail `violations` is given as the
/// array it encodes, and `extra` adds details that are not scalars.
#[must_use]
pub fn error_data(error: &Error, extra: Map<String, Value>) -> Value {
    let arguments: Map<String, Value> = error
        .arguments()
        .iter()
        .map(|(k, v)| ((*k).to_owned(), json_of(v)))
        .collect();
    let mut details: Map<String, Value> = error
        .details()
        .iter()
        .filter(|(k, _)| **k != "panic")
        .map(|(k, v)| {
            let value = match (*k, v) {
                ("violations", arc_base::error::Value::Text(text)) => {
                    serde_json::from_str(text).unwrap_or_else(|_| json_of(v))
                }
                _ => json_of(v),
            };
            ((*k).to_owned(), value)
        })
        .collect();
    details.extend(extra);
    json!({
        "code": error.code(),
        "category": error.category().as_str(),
        "messageId": error.message_id(),
        "arguments": arguments,
        "details": details,
    })
}

/// The JSON-RPC error object of `error`: `code` from [`rpc_code`], `message` the stable error
/// code, and `data` from [`error_data`].
#[must_use]
pub fn error_object(error: &Error, extra: Map<String, Value>) -> Value {
    json!({
        "code": rpc_code(error),
        "message": error.code(),
        "data": error_data(error, extra),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::codes;

    #[test]
    fn every_code_of_the_crate_maps_into_the_table() {
        let table = [
            PARSE_ERROR,
            INVALID_REQUEST,
            METHOD_NOT_FOUND,
            INVALID_PARAMS,
            INTERNAL_ERROR,
            UNKNOWN,
            SCHEMA,
            DISABLED,
            PERMISSION,
            NOT_FOUND,
            CONFLICT,
            CANCELLED,
            LIMIT,
            OUTSIDE_ROOTS,
            FAILED,
            VERSION,
            FAULT,
        ];
        for code in codes::ALL {
            assert!(
                table.contains(&rpc_code(&Error::new(*code))),
                "{}",
                code.code
            );
        }
        assert_eq!(rpc_code(&Error::new(codes::REVISION_MISMATCH)), CONFLICT);
        assert_eq!(rpc_code(&Error::new(codes::CANCELLED)), CANCELLED);
        assert_eq!(rpc_code(&Error::new(codes::FAULT)), FAULT);
        assert_eq!(rpc_code(&Error::new(codes::PERMISSION_DENIED)), PERMISSION);
        assert_eq!(rpc_code(&Error::new(codes::PARAMS_INVALID)), SCHEMA);
        assert_eq!(rpc_code(&Error::new(codes::PARAMS_LIMIT_EXCEEDED)), LIMIT);
        assert_eq!(rpc_code(&Error::new(codes::PATH_IO)), FAILED);
        assert_eq!(rpc_code(&Error::new(codes::DOCUMENT_UNKNOWN)), NOT_FOUND);
    }

    #[test]
    fn panic_text_is_dropped_and_violations_are_structured() {
        let error = Error::new(codes::FAULT).with_detail("panic", "secret /home/x");
        let object = error_object(&error, Map::new());
        assert_eq!(object["data"]["details"], json!({}));
        assert!(!object.to_string().contains("secret"));
        let error = Error::new(codes::PARAMS_INVALID)
            .with_detail("violations", r#"[{"pointer":"/a","kind":"type"}]"#);
        let object = error_object(&error, Map::new());
        assert_eq!(object["data"]["details"]["violations"][0]["pointer"], "/a");
        assert_eq!(object["code"], SCHEMA);
        assert_eq!(object["message"], "cmd.params.invalid");
    }
}
