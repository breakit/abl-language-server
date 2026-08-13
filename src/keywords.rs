//! ABL keyword list for completion in SpeedScript sections.

pub const ABL_KEYWORDS: &[&str] = &[
    "DEFINE", "VARIABLE", "PARAMETER", "BUFFER", "PROPERTY", "TEMP-TABLE", "DATASET",
    "INPUT", "OUTPUT", "INPUT-OUTPUT", "RETURN", "RETURN-VALUE", "AS", "LIKE", "NO-UNDO",
    "NO-SCROLL", "INITIAL", "LABEL", "FORMAT", "EXTENT", "FOR", "EACH", "FIRST", "LAST",
    "BY", "WHERE", "BREAK", "WHEN", "THEN", "ELSE", "END", "IF", "CASE", "REPEAT", "WHILE",
    "DO", "MESSAGE", "RUN", "CALL", "COMPILE", "CAN-FIND", "AVAILABLE", "MESSAGE-LINE",
    "STATEMENT", "FUNCTION", "PROCEDURE", "TRANSACTION", "CREATE", "DELETE", "FIND",
    "ASSIGN", "DISPLAY", "PUT", "GET", "SET", "OUTPUT-TO", "CLOSE", "OPEN", "STREAM",
    "VIEW", "APPLY", "TRIGGER", "ON", "OF", "ANYWHERE", "EXCLUSIVE-LOCK", "SHARE-LOCK",
    "NO-LOCK", "WITH", "NO-LABELS", "FRAME", "DOWN", "UP", "SET-SELECTED", "SELF",
    "SUPER", "NEW", "THIS-OBJECT", "SESSION", "PROPATH", "INITIALIZE", "TERMINATE",
    "QUIT", "EXIT", "NEXT", "&GLOBAL-DEFINE", "&SCOPED-DEFINE", "&UNDEFINE", "&IF",
    "&THEN", "&ELSE", "&ENDIF", "&MESSAGE",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_are_uppercase_ascii() {
        assert!(ABL_KEYWORDS
            .iter()
            .all(|k| k.chars().all(|c| c.is_ascii_uppercase() || c == '-' || c == '&')));
    }
}