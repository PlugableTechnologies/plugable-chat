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
BOLD='\033[1m'
NC='\033[0m' # No Color

# Track what was newly installed (for PATH guidance)
INSTALLED_HOMEBREW=false
INSTALLED_NODE=false
INSTALLED_RUST=false
INSTALLED_ANYTHING=false

# Check if a command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Probe known installation paths and add them to the session PATH
# This handles cases where tools were installed but PATH wasn't updated
probe_known_paths() {
    echo -e "${GRAY}Probing known installation paths...${NC}"
    
    local paths_added=0
    
    # Homebrew - Apple Silicon Macs
    if [[ -f /opt/homebrew/bin/brew ]] && [[ ":$PATH:" != *":/opt/homebrew/bin:"* ]]; then
        echo -e "  ${GRAY}Found Homebrew at: /opt/homebrew${NC}"
        eval "$(/opt/homebrew/bin/brew shellenv)"
        paths_added=$((paths_added + 1))
    fi
    
    # Homebrew - Intel Macs
    if [[ -f /usr/local/bin/brew ]] && [[ ":$PATH:" != *":/usr/local/bin:"* ]]; then
        echo -e "  ${GRAY}Found Homebrew at: /usr/local${NC}"
        eval "$(/usr/local/bin/brew shellenv)"
        paths_added=$((paths_added + 1))
    fi
    
    # Rust/Cargo - user profile location
    if [[ -f "$HOME/.cargo/bin/cargo" ]] && [[ ":$PATH:" != *":$HOME/.cargo/bin:"* ]]; then
        echo -e "  ${GRAY}Found Cargo at: $HOME/.cargo/bin${NC}"
        export PATH="$HOME/.cargo/bin:$PATH"
        paths_added=$((paths_added + 1))
    fi
    
    # Also source cargo env if it exists (sets up all Rust environment vars)
    if [[ -f "$HOME/.cargo/env" ]]; then
        source "$HOME/.cargo/env"
    fi
    
    if [[ $paths_added -gt 0 ]]; then
        echo -e "  ${GREEN}Added $paths_added path(s) to session${NC}"
    else
        echo -e "  ${GRAY}No additional paths needed${NC}"
    fi
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
        INSTALLED_ANYTHING=true
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
    
    # Add Homebrew to PATH for Apple Silicon Macs (for current session)
    if [[ -f /opt/homebrew/bin/brew ]]; then
        echo "  -> Adding Homebrew to PATH for this session..."
        eval "$(/opt/homebrew/bin/brew shellenv)"
        INSTALLED_HOMEBREW=true
    elif [[ -f /usr/local/bin/brew ]]; then
        # Intel Macs - Homebrew is in /usr/local which is usually already in PATH
        eval "$(/usr/local/bin/brew shellenv)"
        INSTALLED_HOMEBREW=true
    fi
    
    if command_exists brew; then
        echo -e "  -> ${GREEN}Installed successfully${NC}"
        INSTALLED_ANYTHING=true
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
        INSTALLED_ANYTHING=true
        
        # Track specific installs for PATH guidance
        if [[ "$formula" == "node" ]]; then
            INSTALLED_NODE=true
        fi
        
        return 0
    else
        echo -e "  -> ${RED}Installation failed${NC}"
        return 1
    fi
}

# Install Rust via rustup if not already installed
# Note: We specifically check for rustup, not just rustc/cargo,
# because rustup is needed to manage targets like wasm32-wasip1
install_rust() {
    echo -n "Checking Rust (rustup)... "
    
    # First, check if rustup is installed
    if command_exists rustup; then
        echo -e "${GREEN}already installed${NC}"
        return 0
    fi
    
    # Check if Rust is installed via Homebrew (without rustup)
    if brew list rust >/dev/null 2>&1; then
        echo -e "${YELLOW}found Homebrew Rust${NC}"
        echo "  -> Homebrew Rust must be uninstalled to use rustup (the official Rust installer)"
        echo "  -> rustup is required for toolchain management (e.g., wasm32-wasip1 target)"
        echo ""
        echo -n "  -> Uninstalling Homebrew Rust... "
        if brew uninstall rust 2>/dev/null; then
            echo -e "${GREEN}done${NC}"
        else
            echo -e "${RED}failed${NC}"
            echo "  -> Please manually run: brew uninstall rust"
            return 1
        fi
    fi
    
    echo -e "  -> ${YELLOW}Installing rustup...${NC}"
    
    # Install via rustup (official method)
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    
    # Source cargo env for current session
    if [[ -f "$HOME/.cargo/env" ]]; then
        echo "  -> Adding Rust to PATH for this session..."
        source "$HOME/.cargo/env"
        INSTALLED_RUST=true
    fi
    
    if command_exists rustup && command_exists rustc && command_exists cargo; then
        echo -e "  -> ${GREEN}Installed successfully${NC}"
        INSTALLED_ANYTHING=true
        return 0
    else
        echo -e "  -> ${RED}Installation failed${NC}"
        return 1
    fi
}

# Install the wasm32-wasip1 target for WASM sandboxing
# Note: wasm32-wasi was renamed to wasm32-wasip1 in Rust 1.78+
install_wasm_target() {
    echo -n "Checking wasm32-wasip1 target... "
    
    if ! command_exists rustup; then
        echo -e "${RED}rustup not found, skipping${NC}"
        return 1
    fi
    
    # Check if target is already installed (check both old and new names)
    if rustup target list --installed 2>/dev/null | grep -qE "wasm32-wasi(p1)?$"; then
        echo -e "${GREEN}already installed${NC}"
        return 0
    fi
    
    echo -e "${YELLOW}installing...${NC}"
    
    if rustup target add wasm32-wasip1; then
        echo -e "  -> ${GREEN}Installed successfully${NC}"
        return 0
    else
        echo -e "  -> ${RED}Installation failed${NC}"
        echo -e "  -> ${GRAY}(WASM sandboxing will be disabled, but Python sandboxing still works)${NC}"
        return 1
    fi
}

# Verify that critical commands are available
verify_commands() {
    local all_available=true
    
    echo ""
    echo "Verifying installations..."
    echo ""
    
    echo -n "  node: "
    if command_exists node; then
        echo -e "${GREEN}$(node --version)${NC}"
    else
        echo -e "${RED}not found${NC}"
        all_available=false
    fi
    
    echo -n "  npm:  "
    if command_exists npm; then
        echo -e "${GREEN}$(npm --version)${NC}"
    else
        echo -e "${RED}not found${NC}"
        all_available=false
    fi
    
    echo -n "  rustc: "
    if command_exists rustc; then
        echo -e "${GREEN}$(rustc --version | cut -d' ' -f2)${NC}"
    else
        echo -e "${RED}not found${NC}"
        all_available=false
    fi
    
    echo -n "  cargo: "
    if command_exists cargo; then
        echo -e "${GREEN}$(cargo --version | cut -d' ' -f2)${NC}"
    else
        echo -e "${RED}not found${NC}"
        all_available=false
    fi
    
    echo -n "  rustup: "
    if command_exists rustup; then
        echo -e "${GREEN}$(rustup --version 2>/dev/null | head -1 | cut -d' ' -f2)${NC}"
    else
        echo -e "${RED}not found${NC}"
        all_available=false
    fi
    
    echo -n "  protoc: "
    if command_exists protoc; then
        echo -e "${GREEN}$(protoc --version | cut -d' ' -f2)${NC}"
    else
        echo -e "${RED}not found${NC}"
        all_available=false
    fi
    
    if $all_available; then
        return 0
    else
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
    
    # Step 4b: wasm32-wasip1 target - Required for WASM sandboxing of Python code execution
    # This is optional but recommended for enhanced security
    install_wasm_target  # Don't fail if this doesn't work
    
    # Step 5: Git (usually pre-installed on macOS, but ensure it's available)
    if ! install_brew_formula "git" "Git"; then
        all_succeeded=false
    fi
    
    # Step 6: Protocol Buffers (protoc) - Required for compiling lance-embedding
    if ! install_brew_formula "protobuf" "Protocol Buffers (protoc)"; then
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
    
    # Always probe known paths before verification (helps on re-runs too)
    echo ""
    probe_known_paths
    
    # Verify all commands are available in current session
    if verify_commands; then
        echo ""
        echo -e "${GREEN}All tools are available in this session!${NC}"
        
        # Check if we're in the project directory (has package.json)
        if [[ -f "package.json" ]]; then
            echo ""
            echo -e "${CYAN}========================================${NC}"
            echo -e "${CYAN}  Running npm install...               ${NC}"
            echo -e "${CYAN}========================================${NC}"
            echo ""
            
            if npm install; then
                echo ""
                echo -e "${GREEN}========================================${NC}"
                echo -e "${GREEN}  Setup complete! Ready to run.        ${NC}"
                echo -e "${GREEN}========================================${NC}"
                echo ""
                
                if $INSTALLED_ANYTHING; then
                    echo -e "  ${YELLOW}NOTE: Tools were just installed.${NC}"
                    echo -e "  ${YELLOW}Open a NEW terminal before running:${NC}"
                    echo ""
                fi
                
                echo -e "  Start the app with:"
                echo -e "  ${YELLOW}npx tauri dev${NC}"
                echo ""
                echo -e "  Or build for production:"
                echo -e "  ${YELLOW}npx tauri build${NC}"
                echo ""
            else
                echo -e "${RED}npm install failed. Please check the errors above.${NC}"
            fi
        else
            echo ""
            echo -e "${CYAN}========================================${NC}"
            echo -e "${CYAN}  Next Steps                           ${NC}"
            echo -e "${CYAN}========================================${NC}"
            echo ""
            
            if $INSTALLED_ANYTHING; then
                echo -e "  ${YELLOW}0. Open a NEW terminal (to pick up PATH changes)${NC}"
            fi
            
            echo "  1. Navigate to the project directory"
            echo -e "  2. Run: ${YELLOW}npm install${NC}"
            echo -e "  3. Run: ${YELLOW}npx tauri dev${NC}"
            echo ""
        fi
    else
        # Some commands are missing - collect which ones
        local missing_tools=""
        command_exists node || missing_tools="$missing_tools node"
        command_exists npm || missing_tools="$missing_tools npm"
        command_exists rustc || missing_tools="$missing_tools rustc"
        command_exists cargo || missing_tools="$missing_tools cargo"
        command_exists rustup || missing_tools="$missing_tools rustup"
        command_exists protoc || missing_tools="$missing_tools protoc"
        
        echo ""
        echo -e "${YELLOW}========================================${NC}"
        echo -e "${YELLOW}  Almost There! Re-run Required        ${NC}"
        echo -e "${YELLOW}========================================${NC}"
        echo ""
        echo -e "${BOLD}The following tools were installed but aren't in PATH yet:${NC}"
        echo ""
        for tool in $missing_tools; do
            echo -e "  ${RED}- $tool${NC}"
        done
        echo ""
        echo -e "${GRAY}This is normal! Your shell needs to be restarted to pick up PATH changes.${NC}"
        echo ""
        echo -e "Please do the following:"
        echo ""
        echo -e "  ${BOLD}1.${NC} Close this terminal window completely"
        echo -e "  ${BOLD}2.${NC} Open a NEW terminal"
        echo -e "  ${BOLD}3.${NC} Re-run this script:"
        echo ""
        echo -e "     ${CYAN}cd \"$(pwd)\"${NC}"
        echo -e "     ${CYAN}./requirements.sh${NC}"
        echo ""
        echo -e "  ${GRAY}The script is safe to run multiple times (idempotent).${NC}"
        echo -e "  ${GRAY}It will skip already-installed packages and continue setup.${NC}"
        echo ""
        
        # Provide manual PATH fix as alternative
        echo -e "${GRAY}Alternatively, you can try refreshing PATH manually in this session:${NC}"
        if [[ -f /opt/homebrew/bin/brew ]]; then
            echo -e "${GRAY}  eval \"\$(/opt/homebrew/bin/brew shellenv)\"${NC}"
        elif [[ -f /usr/local/bin/brew ]]; then
            echo -e "${GRAY}  eval \"\$(/usr/local/bin/brew shellenv)\"${NC}"
        fi
        if [[ -f "$HOME/.cargo/env" ]]; then
            echo -e "${GRAY}  source ~/.cargo/env${NC}"
        fi
        echo ""
    fi
}

# Run the installation
install_requirements
