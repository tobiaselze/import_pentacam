# import_pentacam build system
#
# Usage:
#   make              Build release binary
#   make dist         Build + assemble distributable folder
#   make dist-tar     Build + create .tar.gz archive
#   make clean        Remove build artifacts and dist folder
#
# Environment:
#   ORT_DIR           Path to ONNX Runtime (default: /tmp/onnxruntime-linux-x64-gpu-1.20.1)
#   CUDA_PROVIDER     Set to 0 to build CPU-only dist (default: 1)

ORT_DIR    ?= /tmp/onnxruntime-linux-x64-gpu-1.20.1
CUDA_PROVIDER ?= 1
DIST_DIR   := dist/import_pentacam
VERSION    := $(shell grep '^version' import_pentacam/Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')

# ORT build environment
export ORT_LIB_LOCATION := $(ORT_DIR)/lib
export ORT_PREFER_DYNAMIC_LINK := 1

.PHONY: build dist dist-tar clean

build:
	cargo build --release

dist: build
	@echo "Assembling distribution in $(DIST_DIR) ..."
	@rm -rf $(DIST_DIR)
	@mkdir -p $(DIST_DIR)/models
	@# Binary
	cp target/release/import_pentacam $(DIST_DIR)/
	@# ORT core library
	cp $(ORT_DIR)/lib/libonnxruntime.so.1.20.1 $(DIST_DIR)/
	ln -sf libonnxruntime.so.1.20.1 $(DIST_DIR)/libonnxruntime.so.1
	ln -sf libonnxruntime.so.1 $(DIST_DIR)/libonnxruntime.so
ifeq ($(CUDA_PROVIDER),1)
	@# GPU providers (optional — binary falls back to CPU if missing)
	cp $(ORT_DIR)/lib/libonnxruntime_providers_cuda.so $(DIST_DIR)/
	cp $(ORT_DIR)/lib/libonnxruntime_providers_shared.so $(DIST_DIR)/
endif
	@# OCR models
	cp models/pp-ocrv5_server_det.onnx $(DIST_DIR)/models/
	cp models/en_pp-ocrv5_mobile_rec.onnx $(DIST_DIR)/models/
	cp models/en_ppocrv5_dict.txt $(DIST_DIR)/models/
	@echo ""
	@echo "Distribution ready: $(DIST_DIR)/"
	@echo "  Binary:  $(DIST_DIR)/import_pentacam"
	@echo "  Models:  $(DIST_DIR)/models/"
	@echo "  ORT:     $(DIST_DIR)/libonnxruntime.so.1.20.1"
ifeq ($(CUDA_PROVIDER),1)
	@echo "  GPU:     $(DIST_DIR)/libonnxruntime_providers_cuda.so"
endif
	@echo ""
	@echo "Run:  cd $(DIST_DIR) && ./import_pentacam --help"

dist-tar: dist
	@echo "Creating archive ..."
	cd dist && tar czf import_pentacam-$(VERSION)-linux-x64.tar.gz import_pentacam/
	@echo "Archive: dist/import_pentacam-$(VERSION)-linux-x64.tar.gz"

clean:
	cargo clean
	rm -rf dist/
