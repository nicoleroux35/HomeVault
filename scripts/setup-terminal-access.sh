#!/bin/bash
# HomeVault Terminal Access Setup Script
# This script configures the necessary sudoers rules for terminal access

set -e

echo "=== HomeVault Terminal Access Setup ==="
echo ""

# Check if running as root
if [ "$EUID" -ne 0 ]; then
    echo "❌ This script must be run as root (use sudo)"
    exit 1
fi

echo "✓ Running as root"
echo ""

# Step 1: Check/Create homevault_admin group
echo "Step 1: Checking homevault_admin group..."
if getent group homevault_admin > /dev/null 2>&1; then
    echo "✓ Group homevault_admin already exists"
else
    echo "Creating group homevault_admin..."
    groupadd homevault_admin
    echo "✓ Group homevault_admin created"
fi
echo ""

# Step 2: Check/Create homevault user
echo "Step 2: Checking homevault system user..."
if id homevault > /dev/null 2>&1; then
    echo "✓ User homevault already exists"
else
    echo "Creating system user homevault..."
    useradd -r -s /bin/bash -d /var/lib/homevault -M homevault
    echo "✓ User homevault created"
fi
echo ""

# Step 3: Add current user to homevault_admin group
echo "Step 3: Adding users to homevault_admin group..."
CURRENT_USER="${SUDO_USER:-$USER}"
if [ -n "$CURRENT_USER" ] && [ "$CURRENT_USER" != "root" ]; then
    usermod -aG homevault_admin "$CURRENT_USER"
    echo "✓ User $CURRENT_USER added to homevault_admin group"
else
    echo "⚠ No non-root user detected. Please add users manually:"
    echo "  sudo usermod -aG homevault_admin <username>"
fi
echo ""

# Step 4: Resolve binary paths
echo "Step 4: Resolving binary paths..."
WG=$(which wg 2>/dev/null || echo "/usr/bin/wg")
WG_QUICK=$(which wg-quick 2>/dev/null || echo "/usr/bin/wg-quick")
OPENVPN=$(which openvpn 2>/dev/null || echo "/usr/sbin/openvpn")
PKILL=$(which pkill 2>/dev/null || echo "/usr/bin/pkill")
KILL=$(which kill 2>/dev/null || echo "/usr/bin/kill")
IP=$(which ip 2>/dev/null || echo "/sbin/ip")
SUDO=$(which sudo 2>/dev/null || echo "/usr/bin/sudo")

echo "  wg: $WG"
echo "  wg-quick: $WG_QUICK"
echo "  openvpn: $OPENVPN"
echo "  pkill: $PKILL"
echo "  kill: $KILL"
echo "  ip: $IP"
echo "  sudo: $SUDO"
echo ""

# Step 5: Create sudoers configuration
echo "Step 5: Creating sudoers configuration..."
cat > /etc/sudoers.d/homevault << EOF
# HomeVault sudoers configuration
# Grant full administrative privileges to homevault_admin group members
# Members of this group have the same rights as root/sudo users

# Full sudo access for homevault_admin group members (like sudo/wheel groups)
%homevault_admin ALL=(ALL:ALL) NOPASSWD: ALL

# Terminal access - allow homevault process to launch shells as admin group members
# This enables the web terminal to provide each admin with their own Linux shell session
homevault ALL=(%homevault_admin) NOPASSWD: $SUDO -u * -i

# VPN management - the unprivileged homevault service delegates privileged
# network operations (interface creation, tunnels, stopping root-owned
# openvpn processes, and reading WireGuard state) to root via passwordless sudo.
homevault ALL=(root) NOPASSWD: $WG, $WG_QUICK, $OPENVPN, $PKILL, $KILL, $IP
EOF

echo "✓ Sudoers file created at /etc/sudoers.d/homevault"
echo ""

# Step 6: Set correct permissions
echo "Step 6: Setting file permissions..."
chmod 0440 /etc/sudoers.d/homevault
echo "✓ Permissions set to 0440"
echo ""

# Step 7: Validate sudoers syntax
echo "Step 7: Validating sudoers syntax..."
if visudo -c -f /etc/sudoers.d/homevault; then
    echo "✓ Sudoers configuration is valid"
else
    echo "❌ Sudoers syntax validation failed!"
    echo "Removing invalid configuration..."
    rm -f /etc/sudoers.d/homevault
    exit 1
fi
echo ""

# Step 8: Ensure data directory exists with correct ownership
echo "Step 8: Checking data directory..."
if [ ! -d /var/lib/homevault ]; then
    mkdir -p /var/lib/homevault
    echo "✓ Created /var/lib/homevault"
fi
chown homevault:homevault /var/lib/homevault
echo "✓ Ownership set for /var/lib/homevault"
echo ""

echo "=== Setup Complete ==="
echo ""
echo "📋 Next steps:"
echo "  1. If you added your user to homevault_admin, log out and log back in"
echo "     OR run: newgrp homevault_admin"
echo "  2. Restart the HomeVault service if it's running"
echo "  3. Log in to HomeVault with an admin account"
echo "  4. Navigate to the Terminal application"
echo ""
echo "✅ Terminal access is now configured!"
