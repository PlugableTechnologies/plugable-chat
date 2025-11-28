#!/bin/bash
#
# Requirements installer for macOS
# This script uses Homebrew to check for and install required dependencies
# in an idempotent manner.
#
# Usage: ./requirements.sh
#

set -e

echo ""
echo "========================================"
echo "  Plugable Chat - macOS Requirements   "
echo "========================================"
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
GRAY='\033[0;90m'
NC='\033[0m' # No Color

# Check if a command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Check if Xcode Command Line Tools are installed
check_xcode_clt() {
    xcode-select -p >/dev/null 2>&1
}

# Install Xcode Command Line Tools if needed
install_xcode_clt() {
    echo -n "Checking Xcode Command Line Tools... "
    
    if check_xcode_clt; then
        echo -e "${GREEN}already installed${NC}"
        return 0
    fi
    
    echo -e "${YELLOW}installing...${NC}"
    echo "  -> A dialog will appear to install Xcode Command Line Tools"
    echo "  -> Please click 'Install' and wait for completion"
    
    # Trigger the installation dialog
    xcode-select --install 2>/dev/null || true
    
    # Wait for user to complete installation
    echo ""
    echo -e "${CYAN}Press Enter after Xcode Command Line Tools installation completes...${NC}"
    read -r
    
    if check_xcode_clt; then
        echo -e "  -> ${GREEN}Installed successfully${NC}"
        return 0
    else
        echo -e "  -> ${RED}Installation may have failed. Please install manually.${NC}"
        return 1
    fi
}

# Install Homebrew if needed
install_homebrew() {
    echo -n "Checking Homebrew... "
    
    if command_exists brew; then
        echo -e "${GREEN}already installed${NC}"
        return 0
    fi
    
    echo -e "${YELLOW}installing...${NC}"
    
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    
    # Add Homebrew to PATH for Apple Silicon Macs
    if [[ -f /opt/homebrew/bin/brew ]]; then
        eval "$(/opt/homebrew/bin/brew shellenv)"
    fi
    
    if command_exists brew; then
        echo -e "  -> ${GREEN}Installed successfully${NC}"
        return 0
    else
        echo -e "  -> ${RED}Installation failed${NC}"
        return 1
    fi
}

# Install a Homebrew formula if not already installed
install_brew_formula() {
    local formula=$1
    local display_name=$2
    
    echo -n "Checking $display_name... "
    
    if brew list "$formula" >/dev/null 2>&1; then
        echo -e "${GREEN}already installed${NC}"
        return 0
    fi
    
    echo -e "${YELLOW}installing...${NC}"
    
    if brew install "$formula"; then
        echo -e "  -> ${GREEN}Installed successfully${NC}"
        return 0
    else
        echo -e "  -> ${RED}Installation failed${NC}"
        return 1
    fi
}

# Install Rust via rustup if not already installed
install_rust() {
    echo -n "Checking Rust... "
    
    if command_exists rustc && command_exists cargo; then
        echo -e "${GREEN}already installed${NC}"
        return 0
    fi
    
    echo -e "${YELLOW}installing...${NC}"
    
    # Install via rustup (official method)
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    
    # Source cargo env for current session
    if [[ -f "$HOME/.cargo/env" ]]; then
        source "$HOME/.cargo/env"
    fi
    
    if command_exists rustc && command_exists cargo; then
        echo -e "  -> ${GREEN}Installed successfully${NC}"
        return 0
    else
        echo -e "  -> ${RED}Installation failed${NC}"
        return 1
    fi
}

# Main installation logic
install_requirements() {
    local all_succeeded=true
    
    # Step 1: Xcode Command Line Tools (required for compilation)
    if ! install_xcode_clt; then
        all_succeeded=false
    fi
    
    # Step 2: Homebrew (package manager)
    if ! install_homebrew; then
        echo -e "${RED}ERROR: Homebrew is required for further installations.${NC}"
        exit 1
    fi
    
    echo ""
    echo "Using Homebrew to install dependencies..."
    echo ""
    
    # Step 3: Node.js - Required for frontend build (React/Vite)
    if ! install_brew_formula "node" "Node.js"; then
        all_succeeded=false
    fi
    
    # Step 4: Rust - Required for Tauri backend
    # Using rustup (official installer) instead of Homebrew for better toolchain management
    if ! install_rust; then
        all_succeeded=false
    fi
    
    # Step 5: Git (usually pre-installed on macOS, but ensure it's available)
    if ! install_brew_formula "git" "Git"; then
        all_succeeded=false
    fi
    
    echo ""
    
    if $all_succeeded; then
        echo -e "${GREEN}========================================${NC}"
        echo -e "${GREEN}  All requirements installed!          ${NC}"
        echo -e "${GREEN}========================================${NC}"
    else
        echo -e "${YELLOW}========================================${NC}"
        echo -e "${YELLOW}  Some installations may have failed   ${NC}"
        echo -e "${YELLOW}========================================${NC}"
    fi
    
    echo ""
    echo -e "${CYAN}IMPORTANT: After installation, you may need to:${NC}"
    echo "  1. Restart your terminal to refresh PATH"
    echo "  2. Run 'source ~/.cargo/env' to add Rust to current session"
    echo ""
    echo -e "${CYAN}To verify installations, run:${NC}"
    echo -e "${GRAY}  node --version${NC}"
    echo -e "${GRAY}  npm --version${NC}"
    echo -e "${GRAY}  rustc --version${NC}"
    echo -e "${GRAY}  cargo --version${NC}"
    echo ""
}

# Run the installation
install_requirements

