# Security policy

## Reporting a vulnerability

Please report security problems privately, not in a public issue. On GitHub, open the repository's **Security** tab and click **Report a vulnerability**. Only the maintainers can see the report.

If you don't see that button, open an issue that only asks for a private contact, with no details of the problem, and we'll reply with a way to reach us.

Include what you found, the steps or the file that triggers it, and the tricky3D version and operating system. We'll reply in the report thread, and once a fix is released we'll credit you unless you'd rather stay anonymous.

## Scope

tricky3D is an offline desktop app, so the main risks come from the files it opens. Reports are especially welcome about a mesh, project or repair script file that:

- crashes the app
- makes it read or write files other than the ones you picked
- makes it run code

tricky3D's code should also never connect to the network. If you see it do so, please report that too. (On Windows, Microsoft's WebView2 runtime makes a few connections of its own; the README's Privacy section explains them.)

## Supported versions

Only the latest release gets security fixes.
