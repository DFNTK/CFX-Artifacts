# CFX Artifacts

Herramienta de escritorio para Windows que actualiza la carpeta **artifacts** de FXServer, crea backups, arranca/para el servidor en modo txAdmin y se puede automatizar con una tarea programada. Hecha con **Tauri 2** (Rust + HTML/JS sin framework).

## Cómo funciona

El ejecutable (`df_artifacts.exe`) escanea **la carpeta donde está** y sus subcarpetas (y, como último recurso, hasta 6 carpetas superiores) buscando un directorio que contenga `FXServer.exe` o `citizen/system_resources`. El nombre de la carpeta da igual; si hay varias, gana la llamada `artifacts`, `artifact`, `fxserver`, `fx-server` o `fivem`.

```
F:\servidor\
├── df_artifacts.exe          <- el ejecutable
├── artifacts\                <- se detecta por contenido
│   ├── FXServer.exe
│   ├── citizen\system_resources\{chat,monitor,...}
│   └── df_version.xml        <- lo crea la app (versión instalada)
├── artifacts_backups\        <- backups (configurable)
├── cfx_artifacts_data\       <- ajustes e historial
│   ├── settings.json
│   └── update_log.jsonl
└── txData\
```

### Flujo de actualización

1. Consulta las versiones (jgscripts, changelog oficial de FiveM y la página de descargas).
2. Descarga a la carpeta temporal y verifica tamaño e integridad del archivo (zip o 7z). Si el paquete no contiene `FXServer.exe`, cancela sin tocar nada.
3. Copia en la carpeta temporal las subcarpetas de `citizen/system_resources` que el usuario quiso conservar (por ejemplo `chat` y `monitor`).
4. Si el servidor está en marcha lo para.
5. Mueve la carpeta artifacts actual a un backup completo y copia la nueva versión.
6. Escribe `df_version.xml`, aplica el límite de backups (borra el más antiguo) y registra la operación en el historial.
7. Reinicia el servidor si estaba en marcha (configurable).

Si la copia falla, restaura el backup automáticamente.

## Funciones

- **Artifacts:** versión instalada/recomendada, aviso de builds con fallos, selector de build, selección de carpetas a conservar.
- **Backups:** restaurar, eliminar, número máximo configurable, carpeta en otro disco.
- **Servidor:** iniciar/parar `FXServer.exe` en modo txAdmin con consola en vivo (colores ANSI), abrir txAdmin y crear acceso directo en el escritorio.
- **Tareas:** crea tareas programadas de Windows (diaria y/o al iniciar sesión).
- **Historial:** fecha, versión anterior, versión nueva y resultado.
- **Ajustes:** backups a guardar, formato de descarga (auto/zip/7z), reinicio tras actualizar, carpeta de backups, carpeta de datos de txAdmin, puerto de txAdmin.
- **Idiomas (14):** es (por defecto), en, pt, de, fr, ru, uk, ar (RTL), ro, it, tr, pl, zh, ja.

## Línea de comandos

```
df_artifacts.exe --update [--keep chat,monitor] [--version N]
df_artifacts.exe --check  [--keep chat,monitor] [--version N]
```

| Opción | Efecto |
|---|---|
| `--update` | Actualiza sin preguntar a la versión recomendada (o a `--version N`). Registra el resultado en el historial. |
| `--check` | Comprueba si hay una versión nueva o si la instalada tiene fallos reportados y muestra una ventana Sí/No para decidir. No hace nada si ya estás al día. |
| `--keep a,b` | Carpetas de `system_resources` a conservar. Por defecto, las guardadas en ajustes (`chat`, `monitor`). |
| `--version N` | Build concreto en lugar del recomendado (debe estar en la lista de versiones disponibles). |

La pestaña **Tareas** crea dos tareas programadas que ejecutan `df_artifacts.exe --check`:

- `CFX Artifacts - Comprobar diario`
- `CFX Artifacts - Comprobar al iniciar sesion`

La tarea guarda la ruta del ejecutable: si lo mueves de sitio, vuelve a crearla.

## Compilar

Requisitos: Rust (toolchain estable), Node.js y WebView2 (incluido en Windows 10/11).

```bash
npm install
npx tauri build --no-bundle
```

El resultado queda en `src-tauri/target/release/df_artifacts.exe`. Para desarrollo: `npx tauri dev`.

Para probar fuera de la carpeta real, la variable de entorno `DF_BASE` fuerza la carpeta base que se escanea.

## Estructura del proyecto

```
src-tauri/
├── src/main.rs          Backend: detección, descarga, backups, servidor, tareas, CLI
├── tauri.conf.json      Ventanas (main + splash)
├── capabilities/        Permisos
└── icons/               Generados con `npx tauri icon icon-src.png`
ui/
├── index.html           Interfaz y estilos (tema oscuro estilo FiveM)
├── app.js               Lógica de la interfaz
├── i18n.js              Traducciones
├── splash.html          Pantalla de carga
├── lucide.min.js        Iconos (Lucide)
└── logo.png             Logo
icon-src.png             Icono original de la app
```

## Añadir un idioma

1. En `ui/i18n.js`, añade el par `[código, nombre nativo]` a `LANGS` (y el código a `RTL` si es de derecha a izquierda).
2. Crea el objeto `T.<código>` con todas las claves de `T.en` (si falta alguna se usa el inglés) y añade sus entradas al bloque de claves de acceso directo a txAdmin.
3. Añade el código a la lista permitida de `save_settings` en `main.rs` y, si quieres traducir la ventana de `--check`, una rama en la función `mb()`.

## Errores del backend

Los comandos de Rust devuelven errores como códigos (`E_DOWNLOAD|detalle`, `E_NO_ARTIFACTS`...). La interfaz los traduce con `terr()` usando las claves `e_*` de `i18n.js`. Los códigos desconocidos se muestran sin traducir.

## Servicios externos

Solo se contacta con `artifacts.jgscripts.com`, `changelogs-live.fivem.net` y `runtime.fivem.net`.
