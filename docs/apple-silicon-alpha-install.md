# FilmCraft AI alpha for Apple Silicon Mac

This is an arm64 Mac alpha. It contains the editor and its matching FilmCraft MCP helper.
It is an independent modified build based on FilmCraft by the ArtCraft team, not an official
ArtCraft release. Upstream community/source links identify the original project.
It does not contain an AI account, Codex, personal books, project media or trained assistants.
Windows, Linux and Intel Macs are outside this release's qualification scope.

## Install the app

Save work and quit older FilmCraft builds before installing or opening this alpha. Existing
builds can share application identity and per-user storage; do not run them against the same
profile concurrently. A private test profile is separate from migrating your everyday profile.

1. Open the supplied disk image and drag FilmCraft AI to Applications, or extract the supplied
   app archive and move the app to Applications. Finish copying before opening it.
2. Open FilmCraft AI from Applications. This alpha is locally/ad-hoc signed, not notarized.
   macOS may block first launch. Use the macOS Privacy & Security first-open review only for
   the release you deliberately obtained and verified; do not disable Gatekeeper globally.
3. Use your own project or the generated demo. Save work before testing new workflows.

No developer checkout, fixed control port or separately copied FilmCraft helper is required.
Moving an installed app while Desktop MCP is active is unsupported: disconnect first, move the
app, then recreate its Desktop setup so the external client uses the new helper location.

## Codex connection

Install Codex CLI separately using the current [official installation guide](https://learn.chatgpt.com/docs/codex/cli).
The existing native acceptance baseline uses `codex-cli 0.166.0`. Newer versions need compatible
App Server and voice support; a successful login alone does not establish that compatibility.
For a standard Apple Silicon Homebrew installation, the executable should be discoverable in
`/opt/homebrew/bin`. The app also searches `/usr/local/bin` and its inherited PATH.

In Terminal, run `codex --version`, then `codex login` and complete the browser sign-in.
`codex login status` checks your login. These commands follow OpenAI's
[authentication documentation](https://learn.chatgpt.com/docs/auth).
Use your own eligible account. Plan availability, account limits and service outages are
controlled by the provider. FilmCraft does not include paid usage or store a shared login.

Open Command Console, choose Codex, then Connect. Turn Voice On only when you want microphone
conversation; macOS will request microphone permission. Headphones are recommended for the
qualified interruption workflow. Microphone mute keeps AI speech audible.

## Desktop ChatGPT connection

Choose Desktop ChatGPT and Connect. Configure the external client's MCP entry using the setup
file FilmCraft displays and the bundled helper command it specifies. Continue typing or speaking
in the external chat; FilmCraft does not embed that client's model picker, composer or voice.
Only one provider can own editing at a time. After a reconnect, use the supported explicit
connection-rebind workflow when available; an older chat may have cached an obsolete tool list.
Record the exact error before creating a new conversation. See the release qualification notes for the tested client and recovery boundaries. A successful
connection is not proof of compatibility with every future client update.

## Your data and privacy

Normal app data lives in `~/Library/Application Support/FilmCraft`, separate from the app bundle.
Your projects and imported source media stay in their chosen locations. Back up projects and the
app data folder before migrating an existing installation. Replacing the app does not bundle or
transfer your personal library or assistants to another user.

Imported library documents are local references; they do not train the model. Bounded retrieved
passages and requested captures may be sent to the selected AI. FilmCraft's internal captures
retain no screenshot files, but provider conversation history can retain image data. Do not
assume provider-side deletion merely because FilmCraft has removed local files.

Direct API connections are disabled and unresolved in this alpha. Original-audio music/mix
understanding is not qualified. The upstream editor's startup update check is disabled for the
AI alpha; install reviewed FilmCraft AI releases rather than an unrelated upstream editor update.
