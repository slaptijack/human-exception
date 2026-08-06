fn main() {
    println!("HUMAN EXCEPTION // resistance console");
    println!("No active satellite link. System bootstrap complete.");
}

#[cfg(test)]
mod tests {
    #[test]
    fn project_bootstraps() {
        assert_eq!(env!("CARGO_PKG_NAME"), "human-exception");
    }
}
