Object.defineProperty(globalThis, "document", {
  value: Object.freeze({
    getElementById(id) {
      const node = findElement(String(id));
      if (node < 0) return null;
      return Object.freeze({
        get textContent() { return readText(node); },
        set textContent(value) { writeText(node, String(value)); },
      });
    },
  }),
});
