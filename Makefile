ARTIFACTS_PATH ?= bin
IMAGE_NAME ?= dusk.iso
MODE ?= debug
ARCH ?= x86_64
MEMORY ?= 512M
# In MB
ISO_SIZE ?= 512
QEMU_OPTS ?= 
#MKSQUASHFS_OPTS ?= 
GDB ?= 
CPUS ?= 1
# FAT type
ESP_BITS ?= 32
EXPORT_SYMBOLS = true

ISO_PATH = ${ARTIFACTS_PATH}/iso_root
#INITRAMFS_PATH = ${ARTIFACTS_PATH}/initramfs
IMAGE_PATH = ${ARTIFACTS_PATH}/${IMAGE_NAME}
ESP_IMAGE = ${ARTIFACTS_PATH}/esp.img
CARGO_OPTS = -Zjson-target-spec --target=src/arch/${ARCH}/${ARCH}-unknown-none.json
QEMU_OPTS += -m ${MEMORY} -drive id=hd0,format=raw,file=${IMAGE_PATH}
LIMINE_BOOT_VARIATION = X64
LIMINE_VERSION = v12.5.2
LIMINE_URL = https://github.com/limine-bootloader/limine/releases/download/${LIMINE_VERSION}/limine-binary.zip

KERNEL_FILE = target/${ARCH}-unknown-none/${MODE}/dusk.elf

ifeq (${MODE},release)
	CARGO_OPTS += --release
endif

ifneq (${CPUS},1)
	QEMU_OPTS += -smp ${CPUS}
endif

ifneq (${GDB},)
	QEMU_OPTS += -s -S
endif

ifneq (${KVM},)
	QEMU_OPTS += -accel kvm -cpu host
endif

ifneq (${UEFI},)
	RUN_OPTS := ovmf-${ARCH}
	QEMU_OPTS += -bios ovmf/ovmf-${ARCH}/OVMF.fd
endif

.PHONY: all build

all: build

build: prepare-bin-files compile-bootloader compile-binaries run-scripts build-iso

check: 
		cargo check -Zjson-target-spec

prepare-bin-files:
		# Remove ISO and everything in the bin directory
		rm -f ${IMAGE_PATH}
		rm -rf ${ARTIFACTS_PATH}/*

		# Make bin/ and bin/iso_root
		mkdir -p ${ARTIFACTS_PATH}
		mkdir -p ${ISO_PATH}
		# mkdir -p ${INITRAMFS_PATH}
run-scripts:
		# Place the build ID into the binary so it can be read at runtime
		@HASH=$$(md5sum ${KERNEL_FILE} | cut -c1-12) && \
		sed -i "s/__BUILD_ID__/$${HASH}/" ${KERNEL_FILE}

		#python scripts/initramfs-test.py 100 ${INITRAMFS_PATH}/

copy-iso-files:
		# Limine files
		mkdir -p ${ISO_PATH}/boot/limine
		mkdir -p ${ISO_PATH}/EFI/BOOT

		cp -v limine.conf limine/limine-bios.sys ${ISO_PATH}/boot/limine
		cp -v limine/BOOT${LIMINE_BOOT_VARIATION}.EFI ${ISO_PATH}/EFI/BOOT/

		# OS files
		cp -v ${KERNEL_FILE} ${ISO_PATH}/boot
		#cp -v ${ARTIFACTS_PATH}/initramfs.img ${ISO_PATH}/boot

build-esp: copy-iso-files
		# Create and populate formatted FAT image for ESP partition (130048 1K-blocks = ~127MiB)
		mkfs.fat -F ${ESP_BITS} -C ${ESP_IMAGE} 130048
		mcopy -s -i ${ESP_IMAGE} ${ISO_PATH}/* ::

partition-iso:
		# Make empty disk image
		dd if=/dev/zero of=${IMAGE_PATH} bs=1M count=0 seek=${ISO_SIZE}
ifneq (${UEFI},)
	parted -s ${IMAGE_PATH} mklabel gpt
	parted -s ${IMAGE_PATH} mkpart ESP fat${ESP_BITS} 2048s 262144s
	parted -s ${IMAGE_PATH} set 1 esp on
else
	parted -s ${IMAGE_PATH} mklabel msdos
	parted -s ${IMAGE_PATH} mkpart primary fat${ESP_BITS} 2048s 262144s
	parted -s ${IMAGE_PATH} set 1 boot on
endif

		# Make second partition spanning the rest of the disk
		parted -s ${IMAGE_PATH} mkpart primary 262145s 100%

build-iso: partition-iso build-esp
		# Splice ESP partition into sector 2048 of the disk image
		dd if=${ESP_IMAGE} of=${IMAGE_PATH} bs=512 seek=2048 conv=notrunc
ifeq (${UEFI},)
	# install limine for legacy bios
	./limine/limine bios-install ${IMAGE_PATH}
endif

compile-bootloader:
	@if [ ! -f "limine/.version" ] || [ "$$(cat limine/.version)" != "${LIMINE_VERSION}" ]; then \
		echo "Downloading Limine ${LIMINE_VERSION}..."; \
		rm -rf limine limine-binary limine-binary.zip; \
		curl -fLo limine-binary.zip "${LIMINE_URL}"; \
		unzip -q limine-binary.zip; \
		mv limine-binary limine; \
		rm limine-binary.zip; \
		printf '%s\n' "${LIMINE_VERSION}" > limine/.version; \
	fi
		${MAKE} -C limine

compile-binaries:
		cargo build ${CARGO_OPTS}

ovmf-x86_64:
	mkdir -p ovmf/ovmf-x86_64
	@if [ ! -d "ovmf/ovmf-x86_64/OVMF.fd" ]; then \
		cd ovmf/ovmf-x86_64 && curl -Lo OVMF.fd https://retrage.github.io/edk2-nightly/bin/RELEASEX64_OVMF.fd; \
	fi

# In debug mode, open a terminal and run this command:
# gdb target/x86_64-unknown-none/debug/CappuccinOS.elf -ex "target remote :1234"

run: build ${RUN_OPTS} run-${ARCH}
run-serial: build ${RUN_OPTS} run-${ARCH}-serial

run-x86_64:
	tmux new-session -d -s qemu 'qemu-system-x86_64 ${QEMU_OPTS}'

run-x86_64-serial:
	qemu-system-x86_64 ${QEMU_OPTS} -boot d -display none -serial stdio -monitor none -no-reboot

line-count:
		cloc --quiet --exclude-dir=bin --include-lang=Rust --csv src/ | tail -n 1 | awk -F, '{print $$5}'
clean:
		cargo clean
		rm -rf ${ARTIFACTS_PATH}
		@if [ -d "limine" ]; then ${MAKE} clean -C limine; fi
