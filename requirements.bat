@echo off
REM Requirements installer for Windows
REM This script calls the PowerShell script to install dependencies via winget
REM
REM Usage: Double-click this file, or run from Command Prompt: requirements.bat
REM

echo.
echo This script will install development requirements for Plugable Chat.
echo Please keep this window open until the installation completes.
echo.

powershell.exe -ExecutionPolicy Bypass -File "%~dp0requirements.ps1"

REM If we installed anything that requires a restart, pause so user sees the message
echo.
pause
