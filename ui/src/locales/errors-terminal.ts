import type { Messages } from '../i18n-types.ts'

export default {
  "Некорректный порядок команд терминала.": {"en": "Invalid command sequence", "es": "Secuencia de comandos del terminal no válida"},
  "Некорректное подтверждение вывода терминала.": {"en": "Invalid output acknowledgement", "es": "Confirmación de salida del terminal no válida"},
  "Некорректная команда потока терминала.": {"en": "Invalid terminal command", "es": "Comando del flujo del terminal no válido"},
  "Исполнение терминала недоступно.": {"en": "Terminal execution is unavailable", "es": "La ejecución del terminal no está disponible"},
  "Ввод терминала не принят; проверьте сессию и очередь.": {"en": "Terminal input was not accepted; check session and queue", "es": "No se ha aceptado la entrada; compruebe la sesión y la cola"},
  "Изменение размера терминала не принято.": {"en": "Terminal resize was not accepted", "es": "No se ha aceptado el cambio de tamaño del terminal"},
  "Ввод не подтверждён; проверьте CLI перед повтором.": {"en": "Terminal input was not confirmed; check the CLI before retrying", "es": "Entrada sin confirmar; compruebe el CLI antes de reintentar"},
  "Не удалось подтвердить подключение к терминалу.": {"en": "Terminal service did not authenticate", "es": "No se pudo autenticar la conexión del terminal"},
  "Изменение размера терминала не подтверждено.": {"en": "Terminal size update was not confirmed", "es": "No se ha confirmado el cambio de tamaño del terminal"},
  "Не удалось открыть поток терминала.": {"en": "Could not open the terminal stream.", "es": "No se pudo abrir el flujo del terminal."},
  "Поток терминала не готов. Ввод не отправлен.": {"en": "The terminal stream is not ready. Input was not sent.", "es": "El flujo del terminal no está listo. No se ha enviado la entrada."},
  "Поток терминала закрыт.": {"en": "The terminal stream is closed.", "es": "El flujo del terminal está cerrado."},
  "Некорректный поток терминала.": {"en": "Invalid terminal stream.", "es": "Flujo del terminal no válido."},
  "Соединение терминала прервано. Неподтверждённый ввод не повторяется.": {"en": "Terminal connection interrupted. Unconfirmed input will not be replayed.", "es": "Conexión del terminal interrumpida. No se repetirá la entrada sin confirmar."},
  "Некорректный локальный auth-token": {
    "en": "Invalid local authentication token",
    "es": "Token de autenticación local no válido"
  },
  "Не удалось сохранить локальное подключение": {
    "en": "Could not save the local connection",
    "es": "No se pudo guardar la conexión local"
  },
  "Не удалось прочитать локальный auth-token": {
    "en": "Could not read the local authentication token",
    "es": "No se pudo leer el token de autenticación local"
  },
  "Папка событий должна быть обычной папкой": {
    "en": "The events directory must be a regular directory",
    "es": "El directorio de eventos debe ser un directorio normal"
  },
  "Папка событий принадлежит другому пользователю": {
    "en": "The events directory belongs to another user",
    "es": "El directorio de eventos pertenece a otro usuario"
  },
  "Подменён helper событий": {
    "en": "The event helper has been replaced",
    "es": "El programa auxiliar de eventos ha sido sustituido"
  },
  "Нужен локальный адрес событий": {
    "en": "A local event address is required",
    "es": "Se necesita una dirección local de eventos"
  },
  "Нет канала событий": {
    "en": "No event channel",
    "es": "No hay un canal de eventos"
  },
  "Недоступен канал": {
    "en": "Channel unavailable",
    "es": "Canal no disponible"
  },
  "Канал должен быть приватным": {
    "en": "The channel must be private",
    "es": "El canal debe ser privado"
  },
  "Неверный канал": {
    "en": "Invalid channel",
    "es": "Canal no válido"
  },
  "Неверный провайдер": {
    "en": "Invalid provider",
    "es": "Proveedor no válido"
  },
  "Событие слишком большое": {
    "en": "Event too large",
    "es": "Evento demasiado grande"
  },
  "Неверное событие": {
    "en": "Invalid event",
    "es": "Evento no válido"
  },
  "Сессия недоступна": {
    "en": "Session unavailable",
    "es": "Sesión no disponible"
  },
  "Дополнительная папка недоступна": {
    "en": "Additional folder unavailable",
    "es": "Carpeta adicional no disponible"
  },
  "Путь дополнительной папки изменился": {
    "en": "The additional folder’s path has changed",
    "es": "La ruta de la carpeta adicional ha cambiado"
  },
  "CLI не найден в PATH": {
    "en": "CLI not found in PATH",
    "es": "No se encontró el CLI en PATH"
  },
  "Профиль агента недоступен": {
    "en": "Agent profile unavailable",
    "es": "Perfil del agente no disponible"
  },
  "Не удалось запустить CLI": {
    "en": "Could not start the CLI",
    "es": "No se pudo iniciar el CLI"
  },
  "Нет process ID": {
    "en": "No process ID",
    "es": "No hay un identificador de proceso"
  },
  "Папка изменилась во время запуска CLI": {
    "en": "The folder changed while the CLI was starting",
    "es": "La carpeta cambió durante el inicio del CLI"
  },
  "Дополнительная папка изменилась во время запуска CLI": {
    "en": "The additional folder changed while the CLI was starting",
    "es": "La carpeta adicional cambió durante el inicio del CLI"
  },
  "Нет PTY descriptor": {
    "en": "No PTY descriptor",
    "es": "No hay un descriptor de PTY"
  },
  "Не удалось настроить неблокирующий PTY": {
    "en": "Could not configure the nonblocking PTY",
    "es": "No se pudo configurar el PTY sin bloqueo"
  },
  "Путь должен поддерживать UTF-8": {
    "en": "The path must support UTF-8",
    "es": "La ruta debe admitir UTF-8"
  },
  "Ошибка наблюдения за процессом": {
    "en": "Process monitoring error",
    "es": "Error al supervisar el proceso"
  },
  "Не удалось остановить группу процессов": {
    "en": "Could not stop the process group",
    "es": "No se pudo detener el grupo de procesos"
  },
  "PTY прототип доступен на macOS/Linux; Windows Job Object + ConPTY ещё не проверены": {
    "en": "The PTY prototype is available on macOS/Linux; Windows Job Object + ConPTY have not been verified yet",
    "es": "El prototipo de PTY está disponible en macOS/Linux; Windows Job Object + ConPTY aún no se han verificado"
  },
  "Исполнение завершено": {
    "en": "Execution ended",
    "es": "La ejecución ha finalizado"
  },
  "Не тот источник события": {
    "en": "Unexpected event source",
    "es": "Origen de evento inesperado"
  },
  "Имя сессии уже изменилось. Обновите данные перед сохранением": {
    "en": "The session name has already changed. Refresh before saving",
    "es": "El nombre de la sesión ya ha cambiado. Actualiza los datos antes de guardar"
  },
  "Вложения доступны только для работающей сессии": {
    "en": "Attachments are only available for a running session",
    "es": "Los adjuntos solo están disponibles para una sesión en ejecución"
  },
  "Не удалось проверить исполнение сессии": {
    "en": "Could not verify the session’s execution state",
    "es": "No se pudo comprobar el estado de ejecución de la sesión"
  },
  "CLI уже завершился; вложение не сохранено": {
    "en": "The CLI has already exited; the attachment was not saved",
    "es": "El CLI ya ha finalizado; no se guardó el adjunto"
  },
  "Вложения пока доступны только на macOS/Linux": {
    "en": "Attachments are currently only available on macOS/Linux",
    "es": "Los adjuntos solo están disponibles actualmente en macOS/Linux"
  },
  "Папка сессии недоступна": {
    "en": "Session folder unavailable",
    "es": "Carpeta de la sesión no disponible"
  },
  "Папка не входила в доступ сессии при запуске": {
    "en": "The folder was not included in the session’s access at launch",
    "es": "La carpeta no estaba incluida en los permisos de acceso de la sesión al iniciarla"
  },
  "Папка заменена после запуска CLI; запустите новую сессию": {
    "en": "The folder was replaced after the CLI started; start a new session",
    "es": "La carpeta se sustituyó después de iniciar el CLI; inicia una sesión nueva"
  },
  "Слишком большой ввод; отправьте до 16 KiB за раз": {
    "en": "Input too large; send up to 16 KiB at a time",
    "es": "Entrada demasiado grande; envía como máximo 16 KiB cada vez"
  },
  "Сессия уже завершена": {
    "en": "The session has already ended",
    "es": "La sesión ya ha finalizado"
  },
  "Очередь ввода заполнена или закрыта": {
    "en": "The input queue is full or closed",
    "es": "La cola de entrada está llena o cerrada"
  },
  "Размер терминала вне диапазона": {
    "en": "Terminal size out of range",
    "es": "Tamaño del terminal fuera de rango"
  },
  "Очередь resize заполнена или сессия закрыта": {
    "en": "The resize queue is full or the session is closed",
    "es": "La cola de cambios de tamaño está llena o la sesión está cerrada"
  },
  "Имя вложения не должно содержать путь": {
    "en": "The attachment name must not contain a path",
    "es": "El nombre del adjunto no debe contener una ruta"
  },
  "Нужно имя файла длиной до 240 байт": {
    "en": "A file name of up to 240 bytes is required",
    "es": "Se necesita un nombre de archivo de hasta 240 bytes"
  },
  "Файл превышает лимит 10 MiB": {
    "en": "The file exceeds the 10 MiB limit",
    "es": "El archivo supera el límite de 10 MiB"
  },
  "Не удалось декодировать вложение": {
    "en": "Could not decode the attachment",
    "es": "No se pudo decodificar el adjunto"
  },
  "Недопустимое имя папки вложений": {
    "en": "Invalid attachment folder name",
    "es": "Nombre de carpeta de adjuntos no válido"
  },
  "Папка вложений недоступна или заменена ссылкой": {
    "en": "The attachment folder is unavailable or was replaced with a link",
    "es": "La carpeta de adjuntos no está disponible o se sustituyó por un enlace"
  },
  "Не удалось создать папку вложения": {
    "en": "Could not create the attachment folder",
    "es": "No se pudo crear la carpeta del adjunto"
  },
  "Путь вложения изменился во время загрузки": {
    "en": "The attachment path changed during upload",
    "es": "La ruta del adjunto cambió durante la subida"
  },
  "Файл вложения недоступен": {
    "en": "Attachment file unavailable",
    "es": "Archivo adjunto no disponible"
  },
  "Файл вложения изменился во время загрузки": {
    "en": "The attachment file changed during upload",
    "es": "El archivo adjunto cambió durante la subida"
  },
  "Недопустимое имя файла вложения": {
    "en": "Invalid attachment file name",
    "es": "Nombre de archivo adjunto no válido"
  },
  "Недопустимая сессия вложения": {
    "en": "Invalid attachment session",
    "es": "Sesión del adjunto no válida"
  },
  "Папка сессии недоступна или заменена": {
    "en": "The session folder is unavailable or was replaced",
    "es": "La carpeta de la sesión no está disponible o ha sido sustituida"
  },
  "Папка заменена после запуска CLI": {
    "en": "The folder was replaced after the CLI started",
    "es": "La carpeta se sustituyó después de iniciar el CLI"
  },
  "Путь вложения должен поддерживать UTF-8": {
    "en": "The attachment path must support UTF-8",
    "es": "La ruta del adjunto debe admitir UTF-8"
  },
  "Не удалось создать файл вложения": {
    "en": "Could not create the attachment file",
    "es": "No se pudo crear el archivo adjunto"
  },
  "Вложение должно быть обычным файлом": {
    "en": "The attachment must be a regular file",
    "es": "El adjunto debe ser un archivo normal"
  },
  "Не удалось записать вложение": {
    "en": "Could not write the attachment",
    "es": "No se pudo escribir el adjunto"
  },
  "Не удалось сохранить вложение на диске": {
    "en": "Could not save the attachment to disk",
    "es": "No se pudo guardar el adjunto en el disco"
  }
} satisfies Messages
