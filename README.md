# tbard

Audible desde la terminal: biblioteca, descarga en máxima calidad dividida por
capítulos, reproducción con mpv y sincronización de la posición con la app del
móvil (Whispersync).

## Uso

```sh
tbard login --locale us          # registra tbard como dispositivo (se abre el navegador)
tbard                            # TUI: biblioteca + reproductor
tbard play juramentada           # sin TUI: descarga si falta y reproduce con mpv
tbard download robot salvaje     # solo descargar
tbard library                    # actualizar y listar la biblioteca
tbard sync                       # enviar posiciones pendientes
```

### Teclas de la TUI

| Biblioteca | | Reproductor | |
|---|---|---|---|
| `/` | buscar | `espacio` | pausa |
| `f` | filtro | `←` / `→` | ±30 s |
| `d` | descargar | `[` / `]` | capítulo |
| `⏎` | reproducir | `+` / `-` | velocidad |
| `r` | actualizar | `9` / `0` | volumen |
| `tab` | ir al reproductor | `t` | temporizador |
| `q` | salir | `s` / `tab` | parar / biblioteca |

## Sincronización

- Al empezar se usa la posición más reciente entre la local y la de Audible.
  Si las dos cambiaron y difieren más de 2 min, tbard pregunta.
- Durante la reproducción guarda en local cada 5 s y envía a Audible cada
  2 min, al pausar y al cerrar.
- Sin conexión, las posiciones quedan en cola y se envían al volver la red.

## Archivos

| Ruta | Contenido |
|---|---|
| `~/Audiobooks/<autor>/<título>/` | un `.m4b` por capítulo + `chapters.json` + portada |
| `~/.local/share/tbard/` | `auth.json` (0600) y `tbard.db` (SQLite) |
| `~/.config/tbard/config.toml` | `library_dir = "~/Audiobooks"` |
| `~/.config/tbard/colors.toml` | colores (plantilla de Noctalia, se recarga en vivo) |
| `~/.local/state/tbard/tbard.log` | log (`TBARD_LOG=debug` para más detalle) |

## Nix

```nix
# flake.nix de tu sistema
inputs.tbard.url = "github:abelland-lgtm/tbard";

# home-manager
imports = [ inputs.tbard.homeManagerModules.default ];
programs.tbard = {
  enable = true;
  settings.library_dir = "~/Audiobooks";
};

# Noctalia: colores de la paleta del fondo
theme.templates.user.tbard = {
  input_path = "${inputs.tbard}/noctalia/tbard.toml";
  output_path = "$XDG_CONFIG_HOME/tbard/colors.toml";
};
```

Desarrollo: `nix develop`, `cargo test`, `nix flake check` (incluye
`cargo deny check licenses`).

La API de Audible usada está documentada en [`docs/audible-api.md`](docs/audible-api.md).

## Licencia

[GPL-3.0 o posterior](LICENSE). Copyright (C) 2026 Abelland.
