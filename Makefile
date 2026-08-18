# 只保留仓库路径；平台配置全部写在对应目标里，不设全局 MEM/CPU_NUM。
A := $(abspath $(dir $(firstword $(MAKEFILE_LIST))))
export A

.DEFAULT_GOAL := all

# 初赛/决赛通用 QEMU 内核：运行时读取设备树，单核执行 init_pre.sh，
# 多核执行 init_final.sh。CPU_CAPACITY=256 只限制编译容量，不决定实际核数。
all: kernel-rv kernel-la

kernel-rv:
	mkdir -p build/kernel-rv
	$(MAKE) -C make defconfig build \
		ARCH=riscv64 BUS=mmio MEM=16G CPU_CAPACITY=256 \
		NO_AXSTD=y AX_LIB=axfeat APP_FEATURES=qemu \
		TARGET_DIR=$(A)/target \
		OUT_CONFIG=$(A)/build/kernel-rv/axconfig.toml \
		OUT_DIR=$(A)/build/kernel-rv
	rust-objcopy -I binary -O elf64-littleriscv -B riscv \
		--rename-section .data=.text,alloc,load,readonly,code \
		build/kernel-rv/moss-dev_riscv64-qemu-virt.bin build/kernel-rv/kernel.bin.o
	rust-lld -flavor gnu -m elf64lriscv -T make/kernel-rv-wrap.ld \
		-o kernel-rv build/kernel-rv/kernel.bin.o
	rm -f build/kernel-rv/kernel.bin.o

kernel-la:
	mkdir -p build/kernel-la
	$(MAKE) -C make defconfig build \
		ARCH=loongarch64 BUS=pci MEM=16G CPU_CAPACITY=256 \
		NO_AXSTD=y AX_LIB=axfeat APP_FEATURES=qemu \
		TARGET_DIR=$(A)/target \
		OUT_CONFIG=$(A)/build/kernel-la/axconfig.toml \
		OUT_DIR=$(A)/build/kernel-la
	loongarch64-linux-musl-gcc \
		-DKERNEL_BINARY='"build/kernel-la/moss-dev_loongarch64-qemu-virt.bin"' \
		-c make/kernel-la-image.S -o build/kernel-la/kernel.bin.o
	rust-lld -flavor gnu -m elf64loongarch -T make/kernel-la-wrap.ld \
		-o kernel-la build/kernel-la/kernel.bin.o
	rm -f build/kernel-la/kernel.bin.o

# 现场 QEMU：固定 2 核 2 GiB，编译后直接启动 Shell。
site-rv:
	mkdir -p build/site-rv
	$(MAKE) -C make defconfig build \
		ARCH=riscv64 BUS=mmio MEM=2G CPU_CAPACITY=2 IP= GW= \
		NO_AXSTD=y AX_LIB=axfeat APP_FEATURES="qemu site-init" \
		TARGET_DIR=$(A)/target \
		OUT_CONFIG=$(A)/build/site-rv/axconfig.toml \
		OUT_DIR=$(A)/build/site-rv
	rust-objcopy -I binary -O elf64-littleriscv -B riscv \
		--rename-section .data=.text,alloc,load,readonly,code \
		build/site-rv/moss-dev_riscv64-qemu-virt.bin build/site-rv/kernel.bin.o
	rust-lld -flavor gnu -m elf64lriscv -T make/kernel-rv-wrap.ld \
		-o build/site-rv/kernel-rv build/site-rv/kernel.bin.o
	rm -f build/site-rv/kernel.bin.o
	test -f images/rootfs/site-rv.img
	qemu-system-riscv64 -machine virt -kernel build/site-rv/kernel-rv \
		-m 2G -smp 2 -nographic -snapshot \
		-drive file=images/rootfs/site-rv.img,format=raw,if=none,id=disk \
		-device virtio-blk-device,drive=disk \
		-device virtio-net-device,netdev=net \
		-netdev user,id=net

site-la:
	mkdir -p build/site-la
	$(MAKE) -C make defconfig build \
		ARCH=loongarch64 BUS=pci MEM=2G CPU_CAPACITY=2 IP= GW= \
		NO_AXSTD=y AX_LIB=axfeat APP_FEATURES="qemu site-init" \
		TARGET_DIR=$(A)/target \
		OUT_CONFIG=$(A)/build/site-la/axconfig.toml \
		OUT_DIR=$(A)/build/site-la
	loongarch64-linux-musl-gcc \
		-DKERNEL_BINARY='"build/site-la/moss-dev_loongarch64-qemu-virt.bin"' \
		-c make/kernel-la-image.S -o build/site-la/kernel.bin.o
	rust-lld -flavor gnu -m elf64loongarch -T make/kernel-la-wrap.ld \
		-o build/site-la/kernel-la build/site-la/kernel.bin.o
	rm -f build/site-la/kernel.bin.o
	test -f images/rootfs/site-la.img
	qemu-system-loongarch64 -kernel build/site-la/kernel-la \
		-m 2G -smp 2 -nographic -snapshot -accel tcg,thread=multi \
		-drive file=images/rootfs/site-la.img,format=raw,if=virtio \
		-nic user,model=virtio-net-pci

# 现场实板：固定配置，只编译镜像，不自动启动或传输。
site-vf2:
	mkdir -p build/site-vf2
	$(MAKE) -C make defconfig build \
		ARCH=riscv64 BUS=mmio MEM=4G CPU_CAPACITY=4 \
		MYPLAT=axplat-riscv64-visionfive2 \
		PLAT_CONFIG=$(A)/vendor/axplat-riscv64-visionfive2/axconfig.toml \
		IP=192.168.1.10 GW=192.168.1.2 \
		NO_AXSTD=y AX_LIB=axfeat APP_FEATURES="vf2 site-init" \
		TARGET_DIR=$(A)/target \
		OUT_CONFIG=$(A)/build/site-vf2/axconfig.toml \
		OUT_DIR=$(A)/build/site-vf2
	cp -f build/site-vf2/moss-dev_visionfive2.bin build/site-vf2/moss-vf2.bin

site-2kla:
	mkdir -p build/site-2kla
	$(MAKE) -C make defconfig build \
		ARCH=loongarch64 BUS=mmio MEM=2G CPU_CAPACITY=2 \
		MYPLAT=axplat-loongarch64-ls2k1000 \
		PLAT_CONFIG=$(A)/vendor/axplat-loongarch64-ls2k1000/axconfig.toml \
		IP=192.168.88.2 GW=192.168.88.1 \
		NO_AXSTD=y AX_LIB=axfeat \
		APP_FEATURES="ls2k1000 site-init ls2k1000-shell-mode" \
		TARGET_DIR=$(A)/target \
		OUT_CONFIG=$(A)/build/site-2kla/axconfig.toml \
		OUT_DIR=$(A)/build/site-2kla
	cp -f build/site-2kla/moss-dev_loongarch64-ls2k1000.bin build/site-2kla/moss-2kla.bin

help:
	@printf '%s\n' \
		'make all        -> kernel-rv + kernel-la (DTB selects initial/final init)' \
		'make site-rv    -> build and start RISC-V64 QEMU' \
		'make site-la    -> build and start LoongArch64 QEMU' \
		'make site-vf2   -> build build/site-vf2/moss-vf2.bin' \
		'make site-2kla  -> build build/site-2kla/moss-2kla.bin'

clean:
	rm -rf build kernel-rv kernel-la

docker:
	docker run -it --rm --privileged \
		--platform linux/amd64 \
		-w /work \
		-v $(CURDIR):/work \
		docker.io/zhouzhouyi/os-contest:20260510 \
		/bin/bash

.PHONY: all kernel-rv kernel-la site-rv site-la site-vf2 site-2kla help clean docker
