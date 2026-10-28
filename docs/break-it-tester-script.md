# Break It: tester script

For testers of a release candidate. Each step says what to do and what you should see. If you see something else, that is a bug: press **Copy details for support** (shown with any red or amber message) and send it with the step number. "Expected" is what the build is meant to do, not a promise that it was run on your machine; steps marked **Known open** are not fixed yet and are here so you can reproduce them for us.

Use a normal Windows install of the release candidate, not a development build.

| # | Do this | Expected |
|---|---|---|
| 1 | Open **Settings > Tools** on a fresh install. | No server called `mcp_test_server`. Only servers you added are listed. |
| 2 | **Add Server**, set the command to `nonexistent-tool`, switch it on. | The switch goes back off. A red line on the card says the program was not found and names the command. Closing and reopening Settings still shows it off. |
| 3 | Repeat step 2 with command `cargo` on a computer without Rust. | The red line says to install the Rust toolchain. The server is not saved as on. |
| 4 | **Settings > Tools**, switch a working server on. | It stays on. No **Unsaved** badge. No **Save** button in the window header. |
| 5 | **Settings > Databases**: switch **Enable Database Toolbox** off, leave a source on. | **Refresh** on the source is greyed out and the card says the toolbox is off. |
| 6 | Switch the toolbox on and press **Refresh** without pressing Save first. | The settings are saved, then the tables are read. No "Refresh worked with the toolbox off" case. |
| 7 | In a chat press **+ > Attach Database** before any refresh has run. | The list says why it is empty (no source, toolbox off, nothing switched on, or tables not read yet). When it is "not read yet" there is a **Refresh** button that fills the list. |
| 8 | **Settings > Network > Test connection.** | One row per address, each saying what it is for (search model, chat-model catalog, GPU components, and so on). An HTTP 404 or 400 reads "is expected here". The text above the rows says "reachable" only means the name resolved and a secure connection was made. A loaded search model does not appear as a green success next to a red address. |
| 9 | Block one address with your firewall (for example `cas-bridge.xethub.hf.co`) and run step 8 again. | Only that row is red, with a reason. No row for `cdn-lfs.huggingface.co`. |
| 10 | Narrow the window to about 700 pixels wide and open Settings. | All tabs are visible, wrapping onto a second line. No sideways scroll bar on the tab strip. |
| 11 | Make **Save** appear (change the system prompt), then close Settings without saving and reopen it. | **Save** is gone on reopening. |
| 12 | First start with no models downloaded, on a slow connection. | While the download percentage moves, the top bar is amber ("Still starting up"), never red "Startup failed". It turns red only if the download stops moving for several minutes. |
| 13 | **Known open.** Ask a question that uses a tool, and unplug the network cable (or turn off Wi-Fi) while it answers. Plug it back in and ask again. | Not fixed in this build. The app may close, or later prompts may fail. Send the details, the time you unplugged, and Windows Event Viewer > Windows Logs > Application entries for `plugable-chat.exe`. |

## The Brix box over the network

Run this first, from any computer with Python 3 and no Plugable Chat at all. It tells you whether the box itself is healthy, so a red result later is not blamed on the app.

```
python scripts/mcp_smoke.py --url http://HOST:8787/mcp
```

Expected: one `PASS` line per step (initialize, tools/list, status, selfcheck, match_skills, read_skill, restricted skill absent) and `ALL PASS` at the end. A `FAIL` line names the step and the reason. Add `--ask "which skill covers Amazon SQP?"` for one question to the box's model (takes minutes).

Then add the box in Plugable Chat (**Settings > Tools > Add Server**, HTTP, the same URL), switch it on, and ask for something it can answer. Expected: the tool call appears in the chat and the answer contains no raw `<tool_response>` text.

## What to send back

The step number, what you saw, the screenshot, and **Copy details for support** pasted into the email. For a crash also the Windows Event Viewer entry (step 13).
