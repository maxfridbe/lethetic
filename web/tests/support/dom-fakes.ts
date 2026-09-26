// Minimal stand-ins for DOM objects that do not exist under Node.
export type Mutable<T> = { -readonly [K in keyof T]: T[K] };

/** The KeyboardEvent members read by the application's keyboard handlers. */
export interface FakeKeyboardEvent {
  key: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  isComposing: boolean;
  repeat: boolean;
  defaultPrevented: boolean;
  target: object;
  currentTarget: object;
  preventDefault(): void;
}

export function fakeKeyboardEvent(
  key: string,
  overrides: Partial<FakeKeyboardEvent> = {},
): FakeKeyboardEvent {
  const commandSurface = {};
  return {
    key,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    metaKey: false,
    isComposing: false,
    repeat: false,
    defaultPrevented: false,
    target: commandSurface,
    currentTarget: commandSurface,
    preventDefault() {
      this.defaultPrevented = true;
    },
    ...overrides,
  };
}

/** Handlers only read the members above; Node has no KeyboardEvent to construct. */
export function asKeyboardEvent(event: FakeKeyboardEvent): KeyboardEvent {
  return event as unknown as KeyboardEvent;
}
