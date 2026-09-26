import { SpaApplication } from "./app.js";
import { classifyBootstrapFragment } from "./safety.js";
import { BrowserTransport } from "./transport.js";
export function bootstrap(fragment) {
    const authentication = classifyBootstrapFragment(fragment);
    const root = globalThis.document.getElementById("app");
    if (root === null) {
        throw new Error("The application root is missing.");
    }
    const application = new SpaApplication(root);
    const transport = new BrowserTransport(application);
    application.attachTransport(transport);
    if (authentication.type === "session") {
        void transport.authenticateSession();
        return;
    }
    if (authentication.type === "malformed") {
        application.transportStatus({
            phase: "auth_failed",
            label: "The nonempty controller fragment is malformed",
            attempt: 0,
            retryInMilliseconds: null,
        });
        return;
    }
    void transport.authenticate(authentication.token);
}
