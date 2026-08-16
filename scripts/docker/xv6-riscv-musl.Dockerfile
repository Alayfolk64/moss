FROM alpine:3.22

RUN apk add --no-cache \
        build-base \
        clang \
        lld \
        llvm \
        make \
        patch \
        perl
