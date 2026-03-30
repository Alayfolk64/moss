# Necessary dependencies for the build system.
#
# NOTE:
# Build logic is local-source only. No network fallback is allowed here.
WORKSPACE_DIR := $(abspath $(CURDIR)/..)
VENDOR_CARGO_CONFIG := --config source.crates-io.replace-with=\"vendored-sources\" --config source.vendored-sources.directory=\"$(WORKSPACE_DIR)/vendor\"
TOOLS_TAG := $(shell uname -m)-$(shell uname -s | tr '[:upper:]' '[:lower:]')
TOOLS_ROOT := $(WORKSPACE_DIR)/.tools/$(TOOLS_TAG)
TOOLS_BIN := $(TOOLS_ROOT)/bin
export PATH := $(TOOLS_BIN):$(PATH)
AXCONFIG_GEN := $(TOOLS_BIN)/axconfig-gen

# Resolve the default platform package name used by make/platform.mk.
ifeq ($(MYPLAT),)
ifeq ($(ARCH),x86_64)
_default_plat_package := axplat-x86-pc
else ifeq ($(ARCH),aarch64)
_default_plat_package := axplat-aarch64-qemu-virt
else ifeq ($(ARCH),riscv64)
_default_plat_package := axplat-riscv64-qemu-virt
else ifeq ($(ARCH),loongarch64)
_default_plat_package := axplat-loongarch64-qemu-virt
else
_default_plat_package :=
endif
else
_default_plat_package := $(MYPLAT)
endif

_vendored_plat_config := $(WORKSPACE_DIR)/vendor/$(_default_plat_package)/axconfig.toml

# The build must be able to resolve a local platform config file.
ifeq ($(wildcard $(PLAT_CONFIG)),)
ifeq ($(wildcard $(_vendored_plat_config)),)
$(error Missing local platform config: $(_vendored_plat_config). Set PLAT_CONFIG to a local file or vendor the platform crate into $(WORKSPACE_DIR)/vendor)
endif
endif

# Tool to generate platform configuration files.
ifeq ($(strip $(shell $(AXCONFIG_GEN) --version 2>/dev/null)),)
ifneq ($(strip $(shell command -v axconfig-gen 2>/dev/null)),)
AXCONFIG_GEN := $(shell command -v axconfig-gen 2>/dev/null)
else
ifeq ($(wildcard $(WORKSPACE_DIR)/vendor/axconfig-gen/Cargo.toml),)
$(error Missing local source for axconfig-gen: $(WORKSPACE_DIR)/vendor/axconfig-gen)
endif
$(info Installing missing tool axconfig-gen from local source (offline)...)
$(shell cargo $(VENDOR_CARGO_CONFIG) install --offline --root $(TOOLS_ROOT) --path $(WORKSPACE_DIR)/vendor/axconfig-gen 1>&2)
ifeq ($(strip $(shell $(AXCONFIG_GEN) --version 2>/dev/null)),)
$(error Failed to build axconfig-gen from local source: $(WORKSPACE_DIR)/vendor/axconfig-gen)
endif
endif
endif

# Cargo binutils wrappers: rust-objcopy / rust-objdump.
ifneq ($(strip $(shell $(TOOLS_BIN)/rust-objcopy --version 2>/dev/null)),)
RUST_OBJCOPY := $(TOOLS_BIN)/rust-objcopy
else
RUST_OBJCOPY := $(shell command -v rust-objcopy 2>/dev/null)
endif

ifneq ($(strip $(shell $(TOOLS_BIN)/rust-objdump --version 2>/dev/null)),)
RUST_OBJDUMP := $(TOOLS_BIN)/rust-objdump
else
RUST_OBJDUMP := $(shell command -v rust-objdump 2>/dev/null)
endif

ifeq ($(strip $(RUST_OBJCOPY)),)
ifeq ($(wildcard $(WORKSPACE_DIR)/vendor/cargo-binutils/Cargo.toml),)
$(error Missing required tool: rust-objcopy. Local source not found: $(WORKSPACE_DIR)/vendor/cargo-binutils)
endif
$(info Installing missing tool cargo-binutils from local source (offline)...)
$(shell cargo $(VENDOR_CARGO_CONFIG) install --offline --root $(TOOLS_ROOT) --path $(WORKSPACE_DIR)/vendor/cargo-binutils 1>&2)
RUST_OBJCOPY := $(shell command -v rust-objcopy 2>/dev/null)
ifeq ($(strip $(RUST_OBJCOPY)),)
$(error Failed to resolve rust-objcopy after local offline install)
endif
endif

ifeq ($(strip $(RUST_OBJDUMP)),)
ifeq ($(wildcard $(WORKSPACE_DIR)/vendor/cargo-binutils/Cargo.toml),)
$(error Missing required tool: rust-objdump. Local source not found: $(WORKSPACE_DIR)/vendor/cargo-binutils)
endif
$(info Installing missing tool cargo-binutils from local source (offline)...)
$(shell cargo $(VENDOR_CARGO_CONFIG) install --offline --root $(TOOLS_ROOT) --path $(WORKSPACE_DIR)/vendor/cargo-binutils 1>&2)
RUST_OBJDUMP := $(shell command -v rust-objdump 2>/dev/null)
ifeq ($(strip $(RUST_OBJDUMP)),)
$(error Failed to resolve rust-objdump after local offline install)
endif
endif
