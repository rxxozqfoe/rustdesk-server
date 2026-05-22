.PHONY: install-dev init-db migrate reset-db sqlx-prepare build run check fmt clippy

install-dev:
	@echo "Installing development dependencies..."
	rustup show active-toolchain || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
	cargo install sqlx-cli --version '~0.8' --no-default-features --features sqlite,rustls
	git submodule update --init --recursive
	@echo "Installing pre-commit hook..."
	cp hooks/pre-commit .git/hooks/pre-commit
	chmod +x .git/hooks/pre-commit
	@echo "Done. Run 'make build' to compile."

init-db:
	sqlx database create
	sqlx migrate run

migrate:
	sqlx migrate run

reset-db:
	sqlx database drop -y
	sqlx database create
	sqlx migrate run

sqlx-prepare:
	cargo sqlx prepare

build:
	cargo build

run:
	cargo run

check:
	cargo check

fmt:
	cargo fmt -p hbbs -- --check

clippy:
	cargo clippy -p hbbs --no-deps -- -D warnings
