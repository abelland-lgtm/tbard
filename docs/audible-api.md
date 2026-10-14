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

## Autenticación de las peticiones ✅ confirmado

Con un dispositivo registrado como iPhone (`A2CZJZGLK2JJVM`), `Authorization:
Bearer` + `client-id: 0` falla con `400 "The specified authorization token does
not correspond to the specified Client-ID"`. Lo que funciona es la **firma ADP**
en cada petición:

- `x-adp-token: <adp_token>`
- `x-adp-alg: SHA256withRSA:1.0`
- `x-adp-signature: <base64(firma)>:<fecha>`
  - fecha: UTC `YYYY-MM-DDTHH:MM:SS.ffffffZ`
  - firma: RSA PKCS#1 v1.5 + SHA-256 con `device_private_key` (PEM PKCS#1)
    sobre `MÉTODO\nRUTA?QUERY\nFECHA\nCUERPO\nADP_TOKEN`

No hace falta refrescar el `access_token` para la API de Audible.

## Descarga real ✅ confirmado (*Robot salvaje*, `B0DKXZLL89`)

- `High` concedió `AAX_44_128` (AAC 44,1 kHz a 126 kbps reales, estéreo):
  244 MB por 4 h 25 min.
- El voucher se descifra con SHA-256(`device_type + serial + user_id + asin`)
  → clave = bytes 0..16 e IV = 16..32, AES-128-CBC. El JSON resultante trae
  `key` e `iv` en hex para `ffmpeg -audible_key/-audible_iv`.
- Cortar el AAXC por capítulo con `-ss/-t` delante de `-i` y `-c copy`
  funciona sin archivo intermedio: 86 capítulos en unos 5 s. La desviación
  máxima por capítulo es de 30 ms y la suma total difiere en 0,9 s.
- La fecha `last_updated` puede venir con menos de 3 decimales
  (`02:59:17.94`).
- Los capítulos de este libro sí traen títulos reales ("Capítulo 31: La
  Primera Noche"); Juramentada solo los trae genéricos.

## Alineación con la app ✅ confirmado

El ms global de tbard (`start_offset_ms` del capítulo + posición dentro de su
archivo) coincide con el de la app del móvil: al mismo capítulo y minuto en
tbard, la app muestra el mismo capítulo y minuto. La intro de marca está
incluida en el archivo y en los offsets, así que no hace falta corregirla.

## Pendiente de verificar

- El login nativo (`auth/register`) con una cuenta real.
- Si existe un endpoint de "marcar como terminado".
