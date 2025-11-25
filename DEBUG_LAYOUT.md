# Layout Debug Guide

## 🎯 Purpose
The app now includes a comprehensive layout debugging utility that logs detailed information about window dimensions, document properties, and all DOM elements.

## 📊 When Debug Info is Logged

1. **Automatically on app startup** - Logs 100ms after initial render
2. **Manual trigger** - Press `Ctrl+Shift+L` anytime to get fresh layout info

## 🔍 How to View the Debug Output

The debug information is logged to the **browser DevTools console**, NOT the terminal.

### Access DevTools:

**Option 1: Keyboard Shortcut**
- **Mac**: `Cmd + Option + I`
- **Windows/Linux**: `Ctrl + Shift + I`

**Option 2: Right-click Menu**
1. Right-click anywhere in the app window
2. Select "Inspect Element" or "Inspect"
3. Click on the "Console" tab

## 📋 What Information is Logged

### 1. Window Info
- Inner/outer width and height
- Device pixel ratio
- Screen dimensions

### 2. Document Info
- Scroll dimensions
- Client dimensions
- Offset dimensions

### 3. Key Elements
Specific information about:
- `html` and `body` elements
- `#root` container
- Main app container
- Header bar
- Sidebar and chat area containers

### 4. All Visible Elements
A complete list of every DOM element with dimensions > 0, including:
- Element identifier (tag + id + classes)
- Size (width, height)
- Position (x, y)
- Display and position properties

## 💡 Tips

- The debug info is most useful when viewed in DevTools with the console expanded
- Use the keyboard shortcut `Ctrl+Shift+L` to refresh the layout info after resizing or making changes
- Look for elements that have unexpected dimensions or positions
- Compare the window dimensions with element dimensions to spot overflow issues
