#!/bin/bash
# Setup udisks2 + udiskie automount for external devices (USB, SD cards, etc.)
# This script configures automatic mounting similar to Windows behavior.
# udiskie is a modern replacement for the obsolete udisks-glue (UDisks v1).

set -e

echo "Setting up udisks2 automount..."

# Install required packages — but only the ones actually missing: udisks2 and
# udiskie are already installed by install.sh's required group, so on a normal
# run nothing happens here.
if command -v apt-get &> /dev/null; then
    automount_missing=""
    for pkg in udisks2 udiskie; do
        if ! dpkg-query -W -f='${db:Status-Status}' "$pkg" 2>/dev/null | grep -qx installed; then
            automount_missing="$automount_missing $pkg"
        fi
    done
    if [ -n "$automount_missing" ]; then
        echo "Installing missing automount packages:$automount_missing..."
        apt-get update
        # Intended word-splitting: $automount_missing is a space-separated list.
        # shellcheck disable=SC2086
        apt-get install -y $automount_missing
    fi
elif command -v pacman &> /dev/null; then
    echo "Installing udisks2 and udiskie..."
    pacman -S --noconfirm udisks2 udiskie
elif command -v dnf &> /dev/null; then
    echo "Installing udisks2 and udiskie..."
    dnf install -y udisks2 udiskie
else
    echo "Unsupported package manager. Please install udisks2 and udiskie manually."
    exit 1
fi

# Create udiskie configuration directory
mkdir -p /etc/homevault

# Create udiskie configuration
cat > /etc/homevault/udiskie.conf << 'EOF'
# udiskie automount configuration for HomeVault
# udiskie uses UDisks2 (not the obsolete UDisks v1 used by udisks-glue)

[General]
# Automatically mount newly inserted devices
automount = true

# Use a file browser to open the mounted directory
use_notify_desktop = false

# Do not mount internal devices (SATA/NVMe system disks)
ignore_internal = true

[Mount Options]
# Default mount options applied to all filesystems
default_options = nosuid,nodev,noexec,noatime

# Per-filesystem mount options (appended to defaults)
ntfs_options = rw,uid=1000,gid=1000
vfat_options = rw,uid=1000,gid=1000,utf8=1
exfat_options = rw,uid=1000,gid=1000
ext4_options = rw,noatime
xfs_options = rw,noatime
btrfs_options = rw,noatime
EOF

# Create systemd service for udiskie (runs as the logged-in user via systemd --user
# is not reliable for a headless server; run as root but drop privileges to the
# "homevault" user so mounted media is owned by that user)
cat > /etc/systemd/system/udiskie.service << 'EOF'
[Unit]
Description=udiskie automount daemon (UDisks2)
After=udisks2.service
Documentation=man:udiskie(1)

[Service]
Type=simple
ExecStart=/usr/bin/udiskie -c /etc/homevault/udiskie.conf -n
Restart=on-failure
RestartSec=5

# Environment: tell udiskie to mount under /media/<user>
Environment=UDISKS_USE_UDisks2=1
EOF

# Enable and start udiskie service
systemctl daemon-reload
systemctl enable udiskie.service
systemctl start udiskie.service

# Ensure the homevault user is in plugdev and storage groups for device access
if id "homevault" &>/dev/null; then
    usermod -a -G plugdev,storage,cdrom "homevault" 2>/dev/null || true
fi

# Also add any logged-in user to those groups
USER=$(who | head -n 1 | awk '{print $1}')
if [ -n "$USER" ]; then
    usermod -a -G plugdev,storage,cdrom "$USER" 2>/dev/null || true
fi

echo "udisks2 automount setup completed successfully."
echo "External devices will now be automatically mounted under /media/<user>/"
