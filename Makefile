# Build Options
export ARCH := riscv64
export LOG := warn
export DWARF := n
export MODE := release
export MEMTRACK := n
export RUSTUP_TOOLCHAIN ?= nightly-2025-05-20
RUSTUP ?= rustup
export RUSTFLAGS ?= -Zcrate-attr=feature(unsigned_signed_diff)

# QEMU Options
export BLK := y
export NET := n
export VSOCK := n
export MEM := 1G
export ICOUNT := n
export BUS := mmio

# Generated Options
export A := $(PWD)
export NO_AXSTD := y
export AX_LIB := axfeat
export APP_FEATURES := qemu
TEST_IMAGE_DIR ?= /test-image
RV_TEST_FS_IMG ?= $(TEST_IMAGE_DIR)/sdcard-rv.img
LA_TEST_FS_IMG ?= $(TEST_IMAGE_DIR)/sdcard-la.img
RV_BOOT_DISK_IMG ?= $(A)/disk.img
LA_BOOT_DISK_IMG ?= $(A)/disk-la.img
CARGO_CONFIG_TEMPLATE := $(A)/cargo-config.toml

ifeq ($(MEMTRACK), y)
	APP_FEATURES += starry-api/memtrack
endif

default: build

ensure-cargo-config:
	@mkdir -p $(A)/.cargo
	@cp -f $(CARGO_CONFIG_TEMPLATE) $(A)/.cargo/config.toml

toolchain: ensure-cargo-config
	@$(RUSTUP) default $(RUSTUP_TOOLCHAIN)

KERNEL_NAME := $(notdir $(A))

ROOTFS_URL = https://github.com/Starry-OS/rootfs/releases/download/20260214
ROOTFS_IMG = rootfs-$(ARCH).img

rootfs:
	@if [ ! -f $(ROOTFS_IMG) ]; then \
		echo "Image not found, downloading..."; \
		curl -f -L $(ROOTFS_URL)/$(ROOTFS_IMG).xz -O; \
		xz -d $(ROOTFS_IMG).xz; \
	fi
	@cp $(ROOTFS_IMG) make/disk.img

img:
	@echo -e "\033[33mWARN: The 'img' target is deprecated. Please use 'rootfs' instead.\033[0m"
	@$(MAKE) --no-print-directory rootfs

defconfig justrun: toolchain
	@$(MAKE) -C make $@

makeclean:
	@$(MAKE) -C make makeclean
	@rm -f $(A)/kernel-rv $(A)/kernel-la

clean: makeclean
	@:

build run debug disasm: toolchain defconfig
	@$(MAKE) -C make $@

all: kernel-rv kernel-la

kernel-rv: toolchain
	$(MAKE) ARCH=riscv64 BUS=mmio build
	cp -f $(A)/$(KERNEL_NAME)_riscv64-qemu-virt.elf $(A)/kernel-rv
	cp -f $(A)/$(KERNEL_NAME)_riscv64-qemu-virt.bin $(A)/kernel-rv.bin

kernel-la: toolchain
	$(MAKE) ARCH=loongarch64 BUS=pci build
	cp -f $(A)/$(KERNEL_NAME)_loongarch64-qemu-virt.elf $(A)/kernel-la
	cp -f $(A)/$(KERNEL_NAME)_loongarch64-qemu-virt.bin $(A)/kernel-la.bin

contest-run-rv:
	@test -f "$(if $(TEST_FS_IMG),$(TEST_FS_IMG),$(RV_TEST_FS_IMG))" || (echo "missing image: $(if $(TEST_FS_IMG),$(TEST_FS_IMG),$(RV_TEST_FS_IMG))" && exit 1)
	@$(MAKE) ARCH=riscv64 defconfig
	@BOOT_IMG=""; \
	if [ -f "$(if $(BOOT_DISK_IMG),$(BOOT_DISK_IMG),$(RV_BOOT_DISK_IMG))" ]; then \
		BOOT_IMG="$(if $(BOOT_DISK_IMG),$(BOOT_DISK_IMG),$(RV_BOOT_DISK_IMG))"; \
	fi; \
	$(MAKE) -C make run ARCH=riscv64 BLK=y NET=y BUS=mmio CONTEST_MODE=y TEST_FS_IMG="$(if $(TEST_FS_IMG),$(TEST_FS_IMG),$(RV_TEST_FS_IMG))" BOOT_DISK_IMG="$$BOOT_IMG"

contest-run-la:
	@test -f "$(if $(TEST_FS_IMG),$(TEST_FS_IMG),$(LA_TEST_FS_IMG))" || (echo "missing image: $(if $(TEST_FS_IMG),$(TEST_FS_IMG),$(LA_TEST_FS_IMG))" && exit 1)
	@$(MAKE) ARCH=loongarch64 BUS=pci defconfig
	@BOOT_IMG=""; \
	if [ -f "$(if $(BOOT_DISK_IMG),$(BOOT_DISK_IMG),$(LA_BOOT_DISK_IMG))" ]; then \
		BOOT_IMG="$(if $(BOOT_DISK_IMG),$(BOOT_DISK_IMG),$(LA_BOOT_DISK_IMG))"; \
	fi; \
	$(MAKE) -C make run ARCH=loongarch64 BLK=y NET=y BUS=pci CONTEST_MODE=y TEST_FS_IMG="$(if $(TEST_FS_IMG),$(TEST_FS_IMG),$(LA_TEST_FS_IMG))" BOOT_DISK_IMG="$$BOOT_IMG"

ci-test:
	./scripts/ci-test.py $(ARCH)

# Aliases
rv:
	$(MAKE) ARCH=riscv64 run

la:
	$(MAKE) ARCH=loongarch64 run

vf2:
	$(MAKE) ARCH=riscv64 APP_FEATURES=vf2 MYPLAT=axplat-riscv64-visionfive2 BUS=mmio build

.PHONY: all build run justrun debug disasm clean makeclean kernel-rv kernel-la contest-run-rv contest-run-la toolchain ensure-cargo-config
