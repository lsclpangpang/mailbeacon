/* Shared select-only combobox. The native select remains the form/event source. */
(function () {
  "use strict";
  const controls = new WeakMap();
  let serial = 0, opened = null, committing = false;
  const arrow = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="m5 6.5 3 3 3-3"/></svg>';
  const check = '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4 8 2.5 2.5L12 5"/></svg>';
  const enabled = (option) => option && !option.disabled && !option.hidden && !option.parentElement?.disabled;

  function label(option) {
    return { title: option?.label || "", detail: option?.dataset.description || "" };
  }

  function content(option) {
    const text = document.createElement("span");
    text.className = "select-text";
    const title = document.createElement("span");
    title.className = "select-title";
    const words = label(option);
    title.textContent = words.title;
    text.append(title);
    if (words.detail) {
      const detail = document.createElement("span");
      detail.className = "select-detail";
      detail.textContent = words.detail;
      text.append(detail);
    }
    return text;
  }

  function refresh(select) {
    const control = controls.get(select);
    if (!control) return;
    const { button } = control;
    const option = select.options[select.selectedIndex];
    button.replaceChildren(content(option));
    button.insertAdjacentHTML("beforeend", arrow);
    button.disabled = select.disabled || select.matches(":disabled");
    const name = select.getAttribute("aria-label") || [...select.labels].map((el) => el.textContent.trim()).join(" ") || select.title;
    if (name) button.setAttribute("aria-label", name);
    if (select.hasAttribute("aria-describedby")) button.setAttribute("aria-describedby", select.getAttribute("aria-describedby"));
    const words = label(option);
    button.title = [words.title, words.detail].filter(Boolean).join(" · ");
    if (opened?.control === control && (button.disabled || !select.isConnected)) close();
  }

  function enhance(select) {
    if (controls.has(select)) return controls.get(select);
    if (select.multiple || select.size > 1) return null;
    const wrap = document.createElement("span");
    wrap.className = `select-control ${select.className}`;
    select.before(wrap);
    wrap.append(select);
    const button = document.createElement("button");
    button.type = "button";
    button.className = "select-trigger";
    button.id = `select-trigger-${++serial}`;
    button.setAttribute("role", "combobox");
    button.setAttribute("aria-haspopup", "listbox");
    button.setAttribute("aria-expanded", "false");
    select.classList.add("select-native");
    select.tabIndex = -1;
    select.setAttribute("aria-hidden", "true");
    wrap.append(button);
    const control = { select, button, wrap };
    controls.set(select, control);
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      if (opened?.control === control) close();
      else open(control);
    });
    button.addEventListener("keydown", (event) => onKey(event, control));
    button.addEventListener("blur", () => {
      if (opened?.control === control) close();
      if (select.dataset.fmt && !committing) {
        select.value = "";
        refresh(select);
      }
    });
    refresh(select);
    return control;
  }

  function mount(root = document) {
    if (root.matches?.("select")) enhance(root);
    root.querySelectorAll?.("select").forEach(enhance);
  }

  function close() {
    if (!opened) return;
    const { control, popup } = opened;
    opened = null;
    control.button.setAttribute("aria-expanded", "false");
    control.button.removeAttribute("aria-controls");
    control.button.removeAttribute("aria-activedescendant");
    popup.remove();
  }

  function activate(index) {
    if (!opened || !enabled(opened.control.select.options[index])) return;
    opened.index = index;
    for (const item of opened.popup.children) item.classList.toggle("is-active", +item.dataset.index === index);
    const item = opened.popup.children[index];
    opened.control.button.setAttribute("aria-activedescendant", item.id);
    // Scroll only the listbox, never its sheet or the document.
    if (item.offsetTop < opened.popup.scrollTop) opened.popup.scrollTop = item.offsetTop;
    else if (item.offsetTop + item.offsetHeight > opened.popup.scrollTop + opened.popup.clientHeight) {
      opened.popup.scrollTop = item.offsetTop + item.offsetHeight - opened.popup.clientHeight;
    }
  }

  function open(control) {
    if (control.select.disabled || control.select.matches(":disabled")) return;
    close();
    refresh(control.select);
    // Toolbar mousedown intentionally preserves the editor selection. Focus explicitly.
    control.button.focus({ preventScroll: true });
    const popup = document.createElement("div");
    popup.id = `select-list-${serial}`;
    popup.className = "select-popover";
    popup.setAttribute("role", "listbox");
    popup.setAttribute("aria-label", control.button.getAttribute("aria-label") || control.button.title);
    for (const [index, option] of [...control.select.options].entries()) {
      const item = document.createElement("div");
      item.className = "select-option";
      item.id = `${popup.id}-${index}`;
      item.dataset.index = index;
      item.setAttribute("role", "option");
      item.setAttribute("aria-selected", String(option.selected));
      item.setAttribute("aria-disabled", String(!enabled(option)));
      item.hidden = option.hidden;
      item.title = [label(option).title, label(option).detail].filter(Boolean).join(" · ");
      item.append(content(option));
      item.insertAdjacentHTML("beforeend", check);
      popup.append(item);
    }
    // Keep options inside the modal accessibility tree, outside the sheet's clipping area.
    (control.select.closest('[role="dialog"]') || document.body).append(popup);
    opened = { control, popup, index: -1, typed: "", typedAt: 0 };
    control.button.setAttribute("aria-expanded", "true");
    control.button.setAttribute("aria-controls", popup.id);
    popup.addEventListener("mousedown", (e) => e.preventDefault());
    popup.addEventListener("pointermove", (e) => {
      const item = e.target.closest(".select-option");
      if (item) activate(+item.dataset.index);
    });
    popup.addEventListener("click", (e) => {
      e.stopPropagation();
      const item = e.target.closest(".select-option");
      if (item) choose(+item.dataset.index);
    });
    const r = control.button.getBoundingClientRect();
    popup.style.minWidth = Math.min(r.width, innerWidth - 16) + "px";
    popup.style.maxWidth = Math.min(Math.max(r.width, 320), innerWidth - 16) + "px";
    const below = innerHeight - r.bottom - 14, above = r.top - 14;
    const up = below < Math.min(popup.scrollHeight, 280) && above > below;
    popup.style.maxHeight = Math.max(40, Math.min(320, up ? above : below)) + "px";
    popup.style.left = Math.max(8, Math.min(r.left, innerWidth - popup.offsetWidth - 8)) + "px";
    popup.style.top = Math.max(8, up ? r.top - popup.offsetHeight - 6 : r.bottom + 6) + "px";
    popup.dataset.side = up ? "top" : "bottom";
    const selected = control.select.selectedIndex;
    activate(enabled(control.select.options[selected]) ? selected : [...control.select.options].findIndex(enabled));
  }

  function choose(index) {
    if (!opened) return;
    const { select } = opened.control;
    if (!select.isConnected || select.matches(":disabled")) { close(); return; }
    if (!enabled(select.options[index])) return;
    const changed = select.selectedIndex !== index;
    close();
    select.selectedIndex = index;
    committing = true;
    try {
      if (changed) {
        select.dispatchEvent(new Event("input", { bubbles: true }));
        select.dispatchEvent(new Event("change", { bubbles: true }));
      }
    } finally {
      committing = false;
      if (select.isConnected) refresh(select);
    }
  }

  function onKey(event, control) {
    if (event.isComposing || event.keyCode === 229 || event.metaKey || event.ctrlKey || event.altKey) return;
    const { key } = event;
    if (key === "Tab") { close(); return; }
    if (key === "Escape") {
      if (opened?.control !== control) return;
      event.preventDefault(); event.stopPropagation(); close(); return;
    }
    if (!["ArrowDown", "ArrowUp", "Home", "End", "Enter", " "].includes(key) && key.length !== 1) return;
    event.preventDefault(); event.stopPropagation();
    if (control.button.disabled) return;
    if (opened?.control !== control) {
      open(control);
      if (["Enter", " ", "ArrowDown", "ArrowUp"].includes(key)) return;
    } else if (key === "Enter" || key === " ") { choose(opened.index); return; }
    const options = [...control.select.options];
    const indices = options.flatMap((option, index) => enabled(option) ? [index] : []);
    if (!indices.length) return;
    if (key === "Home") return activate(indices[0]);
    if (key === "End") return activate(indices.at(-1));
    if (key === "ArrowDown" || key === "ArrowUp") {
      const at = indices.indexOf(opened.index);
      return activate(indices[(at + (key === "ArrowDown" ? 1 : -1) + indices.length) % indices.length]);
    }
    const now = Date.now();
    opened.typed = now - opened.typedAt > 700 ? key.toLocaleLowerCase() : opened.typed + key.toLocaleLowerCase();
    opened.typedAt = now;
    const prefix = [...opened.typed].every((char) => char === opened.typed[0]) ? opened.typed[0] : opened.typed;
    const start = prefix.length === 1 ? indices.indexOf(opened.index) + 1 : 0;
    const ordered = indices.slice(start).concat(indices.slice(0, start));
    const match = ordered.find((index) => options[index].label.toLocaleLowerCase().startsWith(prefix));
    if (match !== undefined) activate(match);
  }

  document.addEventListener("focusin", (event) => {
    if (event.target.matches?.("select")) enhance(event.target)?.button.focus({ preventScroll: true });
  }, true);
  document.addEventListener("pointerdown", (event) => {
    if (opened && !opened.popup.contains(event.target) && !opened.control.wrap.contains(event.target)) close();
  }, true);
  document.addEventListener("scroll", (event) => {
    if (opened && !opened.popup.contains(event.target)) close();
  }, true);
  window.addEventListener("resize", close);
  document.addEventListener("change", (event) => {
    if (event.target.matches?.("select")) queueMicrotask(() => refresh(event.target));
  });
  document.addEventListener("reset", (event) => {
    queueMicrotask(() => event.target.querySelectorAll("select").forEach(refresh));
  });
  new MutationObserver((records) => {
    if (opened && !opened.control.select.isConnected) close();
    for (const record of records) {
      for (const node of record.addedNodes) if (node.nodeType === 1) mount(node);
      const target = record.target.nodeType === 1 ? record.target : record.target.parentElement;
      const select = target?.closest("select");
      if (select) {
        // Option indices and labels may have changed while the menu was open.
        if (opened?.control.select === select) close();
        refresh(select);
      }
      if (record.type === "attributes" && record.target.matches?.("fieldset")) {
        record.target.querySelectorAll("select").forEach(refresh);
      }
    }
  }).observe(document.body, { childList: true, characterData: true, subtree: true, attributes: true, attributeFilter: ["disabled", "selected", "label", "aria-label", "aria-describedby"] });
  window.MailBeaconDropdown = { mount, refresh, close, focus: (select) => enhance(select)?.button.focus() };
  mount();
})();
