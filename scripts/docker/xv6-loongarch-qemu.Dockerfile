FROM ubuntu:20.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        libasound2 \
        libcurl4 \
        libepoxy0 \
        libfdt1 \
        libfuse3-3 \
        libgbm1 \
        libgtk-3-0 \
        liblzo2-2 \
        libnuma1 \
        libsdl2-2.0-0 \
        libspice-server1 \
        libusb-1.0-0 \
        libusbredirparser1 \
    && rm -rf /var/lib/apt/lists/*
