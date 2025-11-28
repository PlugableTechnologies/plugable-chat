@echo off
REM Requirements installer for Windows
REM This script calls the PowerShell script to install dependencies via winget

powershell.exe -ExecutionPolicy Bypass -File "%~dp0requirements.ps1"

