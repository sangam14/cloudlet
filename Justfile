set shell := ["/bin/bash", "-uc"]

# Node is build-time only; BoxLite is the only sandbox runtime.
build-console:
  npm --prefix frontend ci
  npm --prefix frontend run build
  cargo build --release -p cloudlet --locked

console:
  ./target/release/cloudlet dashboard

run: console

doctor:
  ./target/release/cloudlet doctor

test-console:
  npm --prefix frontend test
  npm --prefix frontend run build
  cargo fmt --all --check
  cargo test --workspace --locked
  cargo clippy --workspace --all-targets --locked -- -D warnings
