# rustwm

New features and changes are described in GitHub issues; there is no spec. Keep the README brief and put detail in `docs/`.

## Rust

The developer is experienced but new to Rust. Write idiomatic modern Rust without clever code: structs, enums, explicit ownership, small functions, meaningful types and straightforward error propagation. Avoid unnecessary macros, unsafe code, lifetime gymnastics, excessive generics and abstraction for its own sake. Where Smithay needs advanced Rust, isolate it behind clear interfaces. Comments explain why, particularly around Smithay's lifecycle, ownership, and rendering and input integration.

Smithay's API changes over time; prefer its current documentation and examples over old blog posts or code.

Before committing, run `cargo fmt`, `cargo clippy --all-targets` and `cargo test`.
