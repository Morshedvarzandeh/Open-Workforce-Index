/* OWI first-project training controller.
 * The tour may navigate views and explain controls. It never calls a write API,
 * clicks an action, chooses a worker, or changes workflow state.
 */
(function () {
  "use strict";

  const node = document.getElementById("office-tour-data");
  if (!node) return;
  const CONFIG = JSON.parse(node.textContent);
  const STEPS = Array.isArray(CONFIG.steps) ? CONFIG.steps : [];
  const STORAGE_KEY = "owi-office-tour-v1";

  // >>> tour-state >>>
  function initialTourState(version) {
    return { version, status: "new", step: 0 };
  }

  function normalizeTourState(value, version, stepCount) {
    if (!value || typeof value !== "object" || value.version !== version)
      return initialTourState(version);
    const allowed = ["new", "in_progress", "paused", "completed", "skipped"];
    const status = allowed.includes(value.status) ? value.status : "new";
    const raw = Number.isInteger(value.step) ? value.step : 0;
    const step = Math.max(0, Math.min(Math.max(0, stepCount - 1), raw));
    return { version, status, step };
  }

  function reduceTourState(state, action, version, stepCount) {
    const current = normalizeTourState(state, version, stepCount);
    switch (action.type) {
      case "START": return { version, status: "in_progress", step: 0 };
      case "REPLAY": return { version, status: "in_progress", step: 0 };
      case "RESUME": return { ...current, status: "in_progress" };
      case "GO": return { ...current, status: "in_progress",
        step: Math.max(0, Math.min(stepCount - 1, action.step)) };
      case "PAUSE": return { ...current, status: "paused" };
      case "SKIP": return { ...current, status: "skipped" };
      case "COMPLETE": return { ...current, status: "completed",
        step: Math.max(0, stepCount - 1) };
      default: return current;
    }
  }
  // <<< tour-state <<<

  const root = document.getElementById("trainingTour");
  const coach = document.getElementById("tourCoach");
  const spotlight = document.getElementById("tourSpotlight");
  const title = document.getElementById("tourTitle");
  const body = document.getElementById("tourBody");
  const modeTruth = document.getElementById("tourModeTruth");
  const progress = document.getElementById("tourProgress");
  const dots = document.getElementById("tourDots");
  const back = document.getElementById("tourBack");
  const next = document.getElementById("tourNext");
  const skip = document.getElementById("tourSkip");
  const close = document.getElementById("tourClose");
  const launch = document.getElementById("tourLaunch");
  const app = document.querySelector(".app");
  const reducedMotion = matchMedia("(prefers-reduced-motion: reduce)");
  let state = readState();
  let previousFocus = null;
  let activeTarget = null;
  let ready = false;
  let frame = 0;
  const targetObserver = typeof ResizeObserver === "function"
    ? new ResizeObserver(() => positionSpotlight()) : null;

  function storage() {
    try {
      const probe = "__owi_tour_probe__";
      localStorage.setItem(probe, "1");
      localStorage.removeItem(probe);
      return localStorage;
    } catch (_) { return null; }
  }

  function readState() {
    const store = storage();
    if (!store) return initialTourState(CONFIG.version);
    try {
      return normalizeTourState(JSON.parse(store.getItem(STORAGE_KEY)),
        CONFIG.version, STEPS.length);
    } catch (_) { return initialTourState(CONFIG.version); }
  }

  function writeState(nextState) {
    state = normalizeTourState(nextState, CONFIG.version, STEPS.length);
    const store = storage();
    if (store) {
      try { store.setItem(STORAGE_KEY, JSON.stringify(state)); } catch (_) { /* preference only */ }
    }
    updateLauncher();
  }

  function updateLauncher() {
    if (!launch) return;
    const paused = ["paused", "in_progress"].includes(state.status);
    launch.classList.toggle("resume", paused);
    const label = launch.querySelector(".tour-launch-label");
    if (label) label.textContent = paused
      ? `Resume training · ${state.step + 1}/${STEPS.length}`
      : state.status === "completed" ? "Replay training" : "Training tour";
    launch.setAttribute("aria-label", label ? label.textContent : "Training tour");
  }

  function targetFor(step) {
    const candidates = [step.target_anchor, step.fallback_anchor].filter(Boolean);
    for (const anchor of candidates) {
      const target = document.querySelector(`[data-tour-id="${CSS.escape(anchor)}"]`);
      if (!target || target.hidden || target.closest("[hidden]")) continue;
      const rect = target.getBoundingClientRect();
      if (rect.width || rect.height) return target;
    }
    return null;
  }

  function clearTarget() {
    if (activeTarget) {
      if (targetObserver) targetObserver.unobserve(activeTarget);
      activeTarget.classList.remove("tour-target-pulse");
      activeTarget.removeAttribute("aria-describedby");
    }
    activeTarget = null;
  }

  function positionSpotlight() {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(() => {
      if (!activeTarget || root.hidden) return;
      const rect = activeTarget.getBoundingClientRect();
      const pad = 8;
      spotlight.style.left = `${Math.max(5, rect.left - pad)}px`;
      spotlight.style.top = `${Math.max(5, rect.top - pad)}px`;
      spotlight.style.width = `${Math.min(innerWidth - 10, rect.width + pad * 2)}px`;
      spotlight.style.height = `${Math.min(innerHeight - 10, rect.height + pad * 2)}px`;
    });
  }

  function draw() {
    const step = STEPS[state.step];
    if (!step) return;
    clearTarget();
    root.classList.toggle("is-map", step.placement === "center");
    if (typeof switchView === "function") switchView(step.view, { focusContent: false });
    requestAnimationFrame(() => {
      activeTarget = step.placement === "center" ? null : targetFor(step);
      root.classList.toggle("target-missing", step.placement !== "center" && !activeTarget);
      if (activeTarget) {
        activeTarget.classList.add("tour-target-pulse");
        if (targetObserver) targetObserver.observe(activeTarget);
        activeTarget.setAttribute("aria-describedby", "tourBody");
        activeTarget.scrollIntoView({ behavior: reducedMotion.matches ? "auto" : "smooth",
          block: "center", inline: "nearest" });
        positionSpotlight();
      }
      title.textContent = step.title;
      body.textContent = step.body;
      const mode = typeof LIVE === "undefined" || !LIVE ? "static"
        : data().mode === "demo" ? "demo"
          : data().configured === false ? "unconfigured" : "live";
      modeTruth.textContent = mode === "static"
        ? "Read-only preview: training can explain the workflow, but this page cannot create, staff, run, approve, or spend."
        : mode === "unconfigured"
          ? "Setup required: connect a real roster snapshot, price evidence, and exact worker runners before using paid actions."
          : mode === "demo"
            ? "Sample mode: this tour uses an explicitly labelled demo roster. It never presents sample work as your real evidence."
            : "Live local office: the tour explains controls but never creates, staffs, runs, approves, or spends for you.";
      progress.textContent = `Step ${state.step + 1} of ${STEPS.length}`;
      dots.innerHTML = STEPS.map((_, index) => `<i class="tour-dot ${index < state.step
        ? "done" : index === state.step ? "current" : ""}" aria-hidden="true"></i>`).join("");
      back.disabled = state.step === 0;
      next.textContent = state.step === STEPS.length - 1
        ? "Finish training" : (step.next_label || "Next");
      next.focus({ preventScroll: innerWidth > 760 });
    });
  }

  function open(action) {
    if (!STEPS.length || !root) return;
    previousFocus = document.activeElement;
    writeState(reduceTourState(state, action, CONFIG.version, STEPS.length));
    root.hidden = false;
    document.body.classList.add("tour-open");
    if (app) { app.inert = true; app.setAttribute("aria-hidden", "true"); }
    draw();
  }

  function dismiss(action, restore) {
    writeState(reduceTourState(state, action, CONFIG.version, STEPS.length));
    clearTarget();
    root.hidden = true;
    root.classList.remove("is-map", "target-missing");
    document.body.classList.remove("tour-open");
    if (app) { app.inert = false; app.removeAttribute("aria-hidden"); }
    if (restore !== false && previousFocus && typeof previousFocus.focus === "function")
      previousFocus.focus();
  }

  function go(offset) {
    const destination = state.step + offset;
    if (destination >= STEPS.length) {
      dismiss({ type: "COMPLETE" }, false);
      if (typeof switchView === "function") switchView("results");
      launch.focus();
      return;
    }
    writeState(reduceTourState(state, { type: "GO", step: destination },
      CONFIG.version, STEPS.length));
    draw();
  }

  function trapFocus(event) {
    if (root.hidden || event.key !== "Tab") return;
    const controls = [...coach.querySelectorAll("button:not([disabled])")]
      .filter(control => !control.hidden);
    if (!controls.length) return;
    const first = controls[0], last = controls[controls.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault(); last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault(); first.focus();
    }
  }

  launch.addEventListener("click", () => open({ type: state.status === "paused"
    || state.status === "in_progress" ? "RESUME" : "REPLAY" }));
  back.addEventListener("click", () => go(-1));
  next.addEventListener("click", () => go(1));
  skip.addEventListener("click", () => dismiss({ type: "SKIP" }));
  close.addEventListener("click", () => dismiss({ type: "PAUSE" }));
  root.addEventListener("keydown", trapFocus);
  document.addEventListener("keydown", event => {
    if (root.hidden) return;
    if (event.key === "Escape") { event.preventDefault(); dismiss({ type: "PAUSE" }); }
    else if (event.key === "ArrowRight") { event.preventDefault(); go(1); }
    else if (event.key === "ArrowLeft" && state.step) { event.preventDefault(); go(-1); }
  });
  addEventListener("resize", positionSpotlight);
  addEventListener("scroll", positionSpotlight, true);
  if (window.visualViewport) {
    visualViewport.addEventListener("resize", positionSpotlight);
    visualViewport.addEventListener("scroll", positionSpotlight);
  }

  window.OWITrainingTour = {
    ready() {
      if (ready) return;
      ready = true;
      state = readState();
      updateLauncher();
      if (state.status === "new") open({ type: "START" });
    },
    replay() { open({ type: "REPLAY" }); },
    state() { return { ...state }; },
    reducer: reduceTourState,
  };
  updateLauncher();
}());
