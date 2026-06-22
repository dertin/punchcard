/// Returns the configured greeting.
#[must_use]
pub const fn greeting() -> &'static str {
    "hello"
}

#[cfg(test)]
mod tests {
    use super::greeting;

    #[test]
    fn greeting_is_stable() {
        assert_eq!(greeting(), "hello");
    }
}
