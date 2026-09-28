#!/bin/bash
# Kill HomeVault server process

pkill -x homevault && echo "HomeVault server killed" || echo "No HomeVault server running"
