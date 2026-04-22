#!/bin/bash
# compile_cube.sh

# Path to xdg-shell.xml
XDG_SHELL_XML="/usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml"

# 1. Generate xdg-shell client protocol code
wayland-scanner client-protocol $XDG_SHELL_XML -o xdg-shell-client-protocol.c

# 2. Compile the generated code and the main program
# We'll compile everything together in one go for simplicity
gcc -o cube cube.c xdg-shell-client-protocol.c \
    $(pkg-config --cflags --libs wayland-client wayland-egl egl gl) \
    -lm

if [ $? -eq 0 ]; then
    echo "Compilation successful: ./cube"
else
    echo "Compilation failed"
    exit 1
fi
