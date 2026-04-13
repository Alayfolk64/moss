#!/bin/sh
docker run -it --rm --privileged `
  --platform linux/amd64 `
  -w /work `
  -v ${PWD}:/work `
  docker.io/zhouzhouyi/os-contest:20260104 `
  /bin/bash
