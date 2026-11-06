# Using hushphone

- **Start a group** with New group. Only you see the name you give it.
- **Join someone's group** with Join a group. hushphone mints a voucher, over the mixnet or the mesh, and
  shows its code. Give the code to a member of the group over a channel you trust; when they add it with
  Add a member, you join. [vouchers.md](vouchers.md) walks through both sides with
  screenshots.
- **Talk** by holding the microphone button, or Space while it has focus; let go to send. Escape,
  or dragging away, cancels.
- **Radio mode**, the radio button in a group's header, turns the group into a two-way radio:
  voice messages received from then on play by themselves, one at a time in the order they
  arrive, and you hold the big button to talk. A dot marks a voice message not yet played.
  Pressing the button again returns to the chat.
- **Ping**, the bell in a group's header or Ctrl+P, plays a short tone on every member's device
  and shows who pinged; you hear a roger beep. One ping every 3 seconds at most. It is a few
  bytes, not a sound clip, so it suits slow links such as LoRa.
- **Files.** On Linux, hushphone touches one directory of yours, `~/hushphone`. To send a file,
  put it in `~/hushphone/send` first, then press the paperclip and pick it there. Save puts a
  received file in `~/hushphone/received`, never over another. On Android, the paperclip and
  Save open the system's document picker, which hands hushphone that one file.
- **Marks** on your messages: a clock while queued, ✓ once written to the replicas, ✓✓ once a
  member has acknowledged reading them.
- **F10** hides the window's contents at once.
- **Theme** follows the system, and is dark when the system does not say. Settings can fix it to light or dark.
