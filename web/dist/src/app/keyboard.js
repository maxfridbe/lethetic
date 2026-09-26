import { filteredCommands, isEditableTarget, } from "./helpers.js";
export function handlePaletteKeyDown(event, context) {
    const { palette, snapshot } = context.state;
    if (palette === null ||
        snapshot === null ||
        context.state.activePanel !== "command_palette") {
        return;
    }
    if (!event.defaultPrevented &&
        !event.isComposing &&
        !event.repeat &&
        !event.ctrlKey &&
        !event.altKey &&
        !event.metaKey &&
        event.target === event.currentTarget &&
        event.key.length === 1) {
        const accelerator = event.key.toLowerCase();
        const command = snapshot.commands.find((candidate) => candidate.enabled &&
            candidate.accelerator?.toLowerCase() === accelerator);
        if (command !== undefined) {
            event.preventDefault();
            context.actions.invokeCommand(command);
            return;
        }
    }
    const commands = filteredCommands(snapshot, palette.query);
    switch (event.key) {
        case "ArrowDown":
            event.preventDefault();
            context.actions.setPaletteSelection(Math.min(commands.length - 1, palette.selected + 1));
            return;
        case "ArrowUp":
            event.preventDefault();
            context.actions.setPaletteSelection(Math.max(0, palette.selected - 1));
            return;
        case "Home":
            event.preventDefault();
            context.actions.setPaletteSelection(0);
            return;
        case "End":
            event.preventDefault();
            context.actions.setPaletteSelection(Math.max(0, commands.length - 1));
            return;
        case "Enter": {
            event.preventDefault();
            const command = commands[palette.selected];
            if (command?.enabled === true) {
                context.actions.invokeCommand(command);
            }
            return;
        }
        default:
            return;
    }
}
function trapOverlayFocus(event) {
    const traps = Array.from(globalThis.document.querySelectorAll("[data-focus-trap='true']"));
    const overlay = traps[traps.length - 1] ?? null;
    if (overlay === null) {
        return;
    }
    const focusable = Array.from(overlay.querySelectorAll("button:not([disabled]), input:not([disabled]), textarea:not([disabled]), [tabindex='0']")).filter((element) => element.offsetParent !== null);
    if (focusable.length === 0) {
        event.preventDefault();
        return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    const active = globalThis.document.activeElement;
    if (active === null || !overlay.contains(active)) {
        event.preventDefault();
        (event.shiftKey ? last : first)?.focus();
        return;
    }
    if (event.shiftKey && active === first) {
        event.preventDefault();
        last?.focus();
        return;
    }
    if (!event.shiftKey && active === last) {
        event.preventDefault();
        first?.focus();
    }
}
export function handleGlobalKeyDown(event, context) {
    if (event.ctrlKey &&
        !event.altKey &&
        !event.shiftKey &&
        event.key.toLowerCase() === "p") {
        event.preventDefault();
        context.actions.openPalette();
        return;
    }
    if (event.key === "Escape") {
        if (context.state.activePanel !== null) {
            event.preventDefault();
            context.actions.closeOverlay();
        }
        else if (context.state.debuggerDrawerOpen) {
            event.preventDefault();
            context.actions.toggleDebugger();
        }
        else if (context.state.snapshot?.activity.cancellable === true) {
            event.preventDefault();
            context.actions.stop();
        }
        return;
    }
    if (event.ctrlKey &&
        !event.altKey &&
        event.key.toLowerCase() === "c" &&
        !isEditableTarget(event.target) &&
        globalThis.getSelection()?.toString().length === 0) {
        event.preventDefault();
        if (context.state.snapshot?.activity.cancellable === true) {
            context.actions.stop();
        }
        else {
            context.actions.requestQuit();
        }
        return;
    }
    if (event.key === "Tab" &&
        (context.state.activePanel !== null || context.state.debuggerDrawerOpen)) {
        trapOverlayFocus(event);
    }
}
