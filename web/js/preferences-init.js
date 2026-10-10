/* Apply saved appearance before the main stylesheet is painted. */
(function () {
  const root = document.documentElement;
  let theme = "dark";
  let language = "zh-CN";
  try {
    const savedTheme = localStorage.getItem("endlessvibe.theme");
    const savedLanguage = localStorage.getItem("endlessvibe.language");
    if (savedTheme === "light" || savedTheme === "dark") theme = savedTheme;
    if (savedLanguage === "en" || savedLanguage === "zh-CN") language = savedLanguage;
  } catch (_) {
    // Private browsing can disable localStorage. Defaults are always usable.
  }
  root.dataset.theme = theme;
  root.dataset.language = language;
  root.lang = language;
  root.style.colorScheme = theme;
  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) meta.content = theme === "light" ? "#f4f7fc" : "#0b1018";
})();
