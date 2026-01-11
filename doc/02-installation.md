# Installation Guide

This guide will help you install everything you need to run the bot on your computer.

## Step 1: Install Rust

Rust is the programming language this bot is written in. Don't worry - you don't need to learn Rust to use the bot!

### For Windows Users

1. **Download Rust installer:**
   - Go to: https://rustup.rs/
   - Click the "DOWNLOAD RUSTUP-INIT.EXE" button
   - Save the file to your Downloads folder

2. **Run the installer:**
   - Double-click `rustup-init.exe` in your Downloads folder
   - When prompted, press Enter to proceed with default installation
   - The installer will take a few minutes to download and install everything
   - When it says "Press the Enter key to continue", press Enter

3. **Restart your computer:**
   - Close all terminal/command prompt windows
   - Restart your computer (this is important!)

4. **Verify installation:**
   - Open PowerShell or Command Prompt
   - Type: `rustc --version`
   - You should see something like: `rustc 1.75.0 (or higher)`
   - If you see an error, try restarting your computer again

### For Mac Users

1. **Open Terminal** (Press Cmd + Space, type "Terminal", press Enter)

2. **Run this command:**
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

3. **Follow the prompts:**
   - Press Enter when asked about default installation
   - Type `1` and press Enter to proceed

4. **Restart Terminal:**
   - Close and reopen Terminal, or run: `source $HOME/.cargo/env`

5. **Verify installation:**
   ```bash
   rustc --version
   ```
   - You should see a version number like `rustc 1.75.0`

### For Linux Users

1. **Open Terminal**

2. **Run this command:**
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

3. **Follow the prompts:**
   - Press Enter for default installation
   - Type `1` and press Enter

4. **Reload your shell:**
   ```bash
   source $HOME/.cargo/env
   ```

5. **Verify installation:**
   ```bash
   rustc --version
   ```

## Step 2: Download the Bot Code

### Using Git (Recommended)

If you have Git installed:

1. **Open Terminal/PowerShell/Command Prompt**

2. **Navigate to where you want to save the bot:**
   ```bash
   cd Desktop
   ```
   (or wherever you want to save it)

3. **Clone the repository:**
   ```bash
   git clone https://github.com/terauss/Polymarket-Kalshi-Arbitrage-bot.git
   ```

4. **Go into the folder:**
   ```bash
   cd Polymarket-Kalshi-Arbitrage-bot
   ```

### Without Git (Download ZIP)

1. **Go to the GitHub repository:**
   - Visit: https://github.com/terauss/Polymarket-Kalshi-Arbitrage-bot.git
   - Click the green "Code" button
   - Click "Download ZIP"

2. **Extract the ZIP file:**
   - Find the downloaded ZIP file
   - Right-click and choose "Extract All" (Windows) or double-click (Mac/Linux)
   - Remember where you extracted it!

3. **Open Terminal/PowerShell in that folder:**
   - **Windows**: In the extracted folder, right-click in empty space, choose "Open in Terminal" or "Open PowerShell window here"
   - **Mac**: Right-click folder, choose "New Terminal at Folder"
   - **Linux**: Right-click folder, choose "Open Terminal Here"

## Step 3: Install dotenvx (For Running the Bot)

The bot needs a tool called `dotenvx` to read your configuration file.

### For Windows (PowerShell)

```powershell
iwr https://dotenvx.sh/install.ps1 -useb | iex
```

If you get an error about execution policy, run this first:
```powershell
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
```
Then try the install command again.

### For Mac/Linux

```bash
curl -fsSL https://dotenvx.sh/install.sh | sh
```

### Verify dotenvx Installation

Close and reopen your terminal, then run:
```bash
dotenvx --version
```

You should see a version number.

## Step 4: Build the Bot

Now let's compile the bot code so it's ready to run:

1. **Make sure you're in the bot folder:**
   ```bash
   cd Polymarket-Kalshi-Arbitrage-bot
   ```
   (or wherever you saved it)

2. **Build the bot:**
   ```bash
   cargo build --release
   ```

3. **Wait for it to finish:**
   - This will take 5-15 minutes the first time (downloading dependencies)
   - You'll see lots of text scrolling - this is normal!
   - When you see "Finished release [optimized] target(s)", you're done!

## Troubleshooting Installation

### "cargo: command not found"

**Windows:** Restart your computer after installing Rust, then open a NEW PowerShell window.

**Mac/Linux:** Run `source $HOME/.cargo/env` or restart Terminal.

### Build errors or network issues

- Make sure you have internet connection
- Try again - sometimes it's just a temporary network issue
- If it keeps failing, check you have enough disk space (need at least 2GB free)

### "dotenvx: command not found"

- Close and reopen your terminal completely
- Make sure you ran the installation command correctly
- Try the verification command again

## What's Next?

Great! You've installed everything. Now you need to:

1. **[Get your credentials](./03-credentials.md)** - Get API keys from Kalshi and Polymarket
2. **[Set up configuration](./04-configuration.md)** - Tell the bot how to use your accounts

---

**Ready?** Let's get your API keys: [Getting Your Credentials](./03-credentials.md)

