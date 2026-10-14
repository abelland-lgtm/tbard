# API de Audible: notas observadas

Peticiones y respuestas observadas en el spike (2026-10-13, marketplace `us`,
`api.audible.com`). Es la única fuente para implementar tbard: protocolo, no
código de terceros. Sin tokens, vouchers ni datos de la cuenta.

Libro de prueba: *Juramentada* (`B09GX6H7J8`), 151 capítulos, 57 h 39 min.

## Leer posición

`GET 1.0/annotations/lastpositions?asins=<ASIN>[,<ASIN>…]`

```json
{"asin_last_position_heard_annots": [{"asin": "B09GX6H7J8",
  "last_position_heard": {"last_updated": "2026-08-14 21:17:37.114",
                          "position_ms": 6985777, "status": "Exists"}}],
 "response_groups": ["always-returned"]}
```

- `last_updated` está en UTC, sin zona, con formato `YYYY-MM-DD HH:MM:SS.mmm`.

## Escribir posición (Whispersync) ✅ confirmado

`PUT 1.0/lastpositions/<ASIN>`

```json
{"acr": "<acr>", "asin": "<ASIN>", "position_ms": 7200000}
```

- Responde `200` con el cuerpo vacío (`""`).
- Una lectura inmediata devuelve la posición nueva y `last_updated` = ahora.
- La app del móvil mostró 2:00:00 tras el PUT. La posición original se
  restauró con otro PUT.
- `acr` es obligatorio. Sale de `content_reference.acr` en metadata o en
  licenserequest, y es estable por versión del contenido, así que hay que
  guardarlo en la base de datos.

## Metadata (capítulos, acr, última posición)

`GET 1.0/content/<ASIN>/metadata?response_groups=last_position_heard,content_reference,chapter_info`

- `content_metadata.content_reference`: `acr`, `asin`, `codec` (`mp4a.40.2`),
  `content_format` (`M4A_AAX`), `content_size_in_bytes`, `marketplace`, `sku`,
  `version`.
- `content_metadata.chapter_info`: `brandIntroDurationMs` (4086),
  `brandOutroDurationMs` (11145), `is_accurate` (true), `runtime_length_ms`,
  `runtime_length_sec`, `chapters[]`.
  - Cada capítulo: `{title, start_offset_ms, start_offset_sec, length_ms}`.
    Puede llevar `chapters[]` anidado (partes/libros). En este libro no hay
    anidados y los títulos son genéricos ("Capítulo N").
- `content_metadata.last_position_heard`: igual que en lastpositions.

## Códecs disponibles

`GET 1.0/library/<ASIN>?response_groups=media,product_attrs` → `item.available_codecs[]`:

| name | enhanced_codec |
|---|---|
| `aax_22_64` | `LC_64_22050_stereo` |
| `aax_22_32` | `LC_32_22050_stereo` |
| `aax` | `aax` |

El bitrate está en `enhanced_codec` (`LC_<kbps>_<Hz>_<canales>`), que sirve
para mostrar la calidad máxima disponible.

## Licencia y descarga

`POST 1.0/content/<ASIN>/licenserequest`

```json
{"consumption_type": "Download", "supported_drm_types": ["Adrm", "Mpeg"],
 "quality": "High", "response_groups": "content_reference,chapter_info"}
```

| quality | content_format concedido | tamaño | kbps |
|---|---|---|---|
| `High` | `AAX_22_64` | 1 678 137 467 B | 64 |
| `Normal` | `AAX_22_32` | 847 875 394 B | 32 |

- `High` concede la mejor de `available_codecs`, así que **siempre se pide
  `High`**.
- `content_license`: `status_code` (`Granted`), `drm_type` (`Adrm`),
  `license_response` (voucher cifrado),
  `content_metadata.content_url.offline_url`,
  `content_metadata.content_reference`, `content_metadata.chapter_info`.
- El `chapter_info` de la licencia coincide con el de metadata (151
  capítulos).

## Pendiente de verificar

- El descifrado del voucher (`license_response`) y la descarga real de
  `offline_url` (Fase 2).
- Si el ms del archivo descargado coincide con el de la app. Se comprueba con
  el inicio de un capítulo conocido (Fase 2).
- El protocolo del login nativo (`auth/register`) y del refresh
  (`auth/token`) (Fase 2).
- Si existe un endpoint de "marcar como terminado".
