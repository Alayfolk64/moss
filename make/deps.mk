# Necessary dependencies for the build system.
#
# NOTE:
# Avoid silent, parse-time `cargo install` by default because it can trigger
# long network downloads with no live output (looks like Make is hanging).
# Set `AUTO_INSTALL_DEPS=y` if you still want automatic installation.

AUTO_INSTALL_DEPS ?= n

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

_vendored_plat_config := $(PWD)/vendor/$(_default_plat_package)/axconfig.toml

# `cargo-axplat` is only needed when no local platform config can be resolved.
ifeq ($(wildcard $(PLAT_CONFIG)),)
ifeq ($(wildcard $(_vendored_plat_config)),)
ifeq ($(shell cargo axplat --version 2>/dev/null),)
ifeq ($(AUTO_INSTALL_DEPS),y)
$(info Installing cargo-axplat...)
$(shell cargo install --locked cargo-axplat)
else
$(error Missing required tool: cargo-axplat. Install it manually, or rerun with AUTO_INSTALL_DEPS=y)
endif
endif
endif
endif

# Tool to generate platform configuration files.
ifeq ($(shell axconfig-gen --version 2>/dev/null),)
ifeq ($(AUTO_INSTALL_DEPS),y)
$(info Installing axconfig-gen...)
$(shell cargo install --locked axconfig-gen)
else
$(error Missing required tool: axconfig-gen. Install it manually, or rerun with AUTO_INSTALL_DEPS=y)
endif
endif

# Cargo binutils wrappers: rust-objcopy / rust-objdump.
ifeq ($(shell cargo install --list | grep cargo-binutils),)
ifeq ($(AUTO_INSTALL_DEPS),y)
$(info Installing cargo-binutils...)
$(shell cargo install --locked cargo-binutils)
else
$(error Missing required tool: cargo-binutils. Install it manually, or rerun with AUTO_INSTALL_DEPS=y)
endif
endif
