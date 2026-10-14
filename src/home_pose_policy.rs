//! Home intentionally rejects pre-registry pose consumers until migrated.
//! No coercion, inferred frames, or legacy fallback is permitted.
pub fn unsupported(feature: &str) -> anyhow::Result<()> {
    anyhow::bail!("{feature} disabled: legacy pose updates are unsupported; registry-backed compact pose contract required")
}
#[cfg(test)]
mod tests {
    #[test]
    fn legacy_consumers_fail_closed() {
        for feature in ["camera AR","phone presence","robot presence","depth fusion","robot mapping","camera following"] {
            let error=super::unsupported(feature).unwrap_err().to_string();
            assert!(error.contains("disabled") && error.contains("compact pose"));
        }
    }
}
