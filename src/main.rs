mod cli;

fn main() {
    cli::run();
}

#[cfg(test)]
mod tests {
    #[test]
    fn project_bootstraps() {
        assert_eq!(env!("CARGO_PKG_NAME"), "human-exception");
    }
}
