#!/bin/sh
# On hybrid laptops keep the UI on the integrated GPU: a woken NVIDIA drains the battery.
for v in /sys/class/drm/card*/device/vendor; do
    case "$(cat "$v" 2>/dev/null)" in
        0x1002|0x8086)
            [ -e /usr/share/glvnd/egl_vendor.d/50_mesa.json ] &&
                export __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json
            for icd in radeon intel; do
                [ -e "/usr/share/vulkan/icd.d/${icd}_icd.x86_64.json" ] &&
                    export VK_DRIVER_FILES="/usr/share/vulkan/icd.d/${icd}_icd.x86_64.json"
            done
            break
            ;;
    esac
done
exec /opt/salvage/salvage "$@"
