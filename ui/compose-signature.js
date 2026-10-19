/* Keep an editable, automatically inserted signature separate from quoted text. */
(function (root) {
  "use strict";
  const block = (signature) => signature ? "\n\n" + signature.replace(/\r\n?/g, "\n") : "";

  function createSignatureDraft(body, signature) {
    const inserted = block(signature);
    return { text: inserted + body, start: 0, end: inserted.length, managed: true };
  }

  function editSignatureDraft(draft, text, edit) {
    if (text === draft.text) return draft;
    if (!draft.managed) return { ...draft, text };
    let first = 0;
    while (first < text.length && first < draft.text.length && text[first] === draft.text[first]) first++;
    let oldEnd = draft.text.length, newEnd = text.length;
    while (oldEnd > first && newEnd > first && draft.text[oldEnd - 1] === text[newEnd - 1]) { oldEnd--; newEnd--; }
    // beforeinput gives the actual caret range when identical newlines make a
    // text-only diff ambiguous. Validate it before relying on it (IME/undo).
    if (edit) {
      const candidateEnd = edit.end + text.length - draft.text.length;
      if (candidateEnd >= edit.start && draft.text.slice(0, edit.start) === text.slice(0, edit.start) &&
          draft.text.slice(edit.end) === text.slice(candidateEnd)) {
        first = edit.start;
        oldEnd = edit.end;
        newEnd = candidateEnd;
      }
    }
    const delta = newEnd - oldEnd;
    if (oldEnd <= draft.start) {
      return { ...draft, text, start: draft.start + delta, end: draft.end + delta };
    }
    if (first > draft.end) return { ...draft, text };
    // Once a user edits/removes the inserted signature, it belongs to their draft.
    return { ...draft, text, managed: false };
  }

  function switchSignature(draft, signature) {
    if (!draft.managed) return draft;
    const inserted = block(signature);
    return {
      ...draft,
      text: draft.text.slice(0, draft.start) + inserted + draft.text.slice(draft.end),
      end: draft.start + inserted.length,
    };
  }

  const api = { createSignatureDraft, editSignatureDraft, switchSignature };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else {
    root.MailBeaconSignature = api;
    root.MailPulseSignature = api;
  }
})(typeof globalThis !== "undefined" ? globalThis : this);
