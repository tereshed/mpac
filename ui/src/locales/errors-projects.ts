import type { Messages } from '../i18n-types.ts'

export default {
  "Для профиля доступа нужен UUID новой сессии": {
    "en": "The access profile requires a new session UUID",
    "es": "El perfil de acceso requiere el UUID de una nueva sesión"
  },
  "Этот режим доступа недоступен для выбранного агента": {
    "en": "This access mode is not available for the selected agent",
    "es": "Este modo de acceso no está disponible para el agente seleccionado"
  },
  "Список дополнительных папок не соответствует выбранным папкам проекта": {
    "en": "The additional folders do not match the selected project folders",
    "es": "Las carpetas adicionales no coinciden con las carpetas del proyecto seleccionadas"
  },
  "Дополнительная папка пуста или указана дважды": {
    "en": "An additional folder is empty or listed twice",
    "es": "Hay una carpeta adicional vacía o duplicada"
  },
  "Дополнительные папки должны иметь разные абсолютные пути без NUL": {
    "en": "Additional folders must have distinct absolute paths without NUL characters",
    "es": "Las carpetas adicionales deben tener rutas absolutas distintas y sin caracteres NUL"
  },
  "Подтверждение полного доступа применимо только к режиму полного доступа": {
    "en": "Full-access approval applies only to full-access mode",
    "es": "La confirmación de acceso completo solo se aplica al modo de acceso completo"
  },
  "Для полного доступа нужно явно подтвердить риск": {
    "en": "You must explicitly acknowledge the risk to use full access",
    "es": "Debes aceptar expresamente el riesgo para usar el acceso completo"
  },
  "Для настроек подтверждений, сети или дополнительных папок выберите явный режим доступа": {
    "en": "Choose an explicit access mode to configure approvals, networking or additional folders",
    "es": "Elige un modo de acceso explícito para configurar las confirmaciones, la red o las carpetas adicionales"
  },
  "В выбранном режиме дополнительные папки не поддерживаются": {
    "en": "Additional folders are not supported in the selected mode",
    "es": "El modo seleccionado no admite carpetas adicionales"
  },
  "Настройка сети доступна только в режиме Codex workspace-write": {
    "en": "Network settings are available only in Codex workspace-write mode",
    "es": "Los ajustes de red solo están disponibles en el modo workspace-write de Codex"
  },
  "Codex read-only не запрашивает расширение прав; выберите never": {
    "en": "Codex read-only does not request elevated permissions; choose never",
    "es": "El modo read-only de Codex no solicita permisos adicionales; elige never"
  },
  "Для workspace-write нужен UUID новой сессии": {
    "en": "workspace-write requires a new session UUID",
    "es": "El modo workspace-write requiere el UUID de una nueva sesión"
  },
  "Для Claude Code отдельные настройки approval и network не поддерживаются": {
    "en": "Claude Code does not support separate approval and network settings",
    "es": "Claude Code no admite ajustes separados de approval y network"
  },
  "Для Hermes отдельные настройки approval и network не поддерживаются": {
    "en": "Hermes does not support separate approval and network settings",
    "es": "Hermes no admite ajustes separados de approval y network"
  },
  "Этот агент не поддерживает выбранный режим доступа": {
    "en": "This agent does not support the selected access mode",
    "es": "Este agente no admite el modo de acceso seleccionado"
  },
  "Домашняя папка не определена": {
    "en": "The home folder is not defined",
    "es": "No se ha definido la carpeta personal"
  },
  "Домашняя папка недоступна": {
    "en": "The home folder is unavailable",
    "es": "La carpeta personal no está disponible"
  },
  "Для навигации нужен абсолютный путь": {
    "en": "An absolute path is required for navigation",
    "es": "Se necesita una ruta absoluta para navegar"
  },
  "Путь слишком длинный": {
    "en": "The path is too long",
    "es": "La ruta es demasiado larga"
  },
  "Папка не найдена или недоступна": {
    "en": "The folder was not found or is unavailable",
    "es": "No se ha encontrado la carpeta o no está disponible"
  },
  "Выберите папку, а не файл": {
    "en": "Choose a folder, not a file",
    "es": "Elige una carpeta, no un archivo"
  },
  "Нет доступа к содержимому папки": {
    "en": "Access to the folder contents is denied",
    "es": "No tienes acceso al contenido de la carpeta"
  },
  "Путь не поддерживает UTF-8": {
    "en": "The path is not valid UTF-8",
    "es": "La ruta no es válida en UTF-8"
  },
  "Нет пути локальной базы": {
    "en": "The local database path is missing",
    "es": "Falta la ruta de la base de datos local"
  },
  "Нет папки локальной базы": {
    "en": "The local database folder is missing",
    "es": "Falta la carpeta de la base de datos local"
  },
  "Проект не найден или удалён": {
    "en": "The project was not found or has been deleted",
    "es": "No se ha encontrado el proyecto o se ha eliminado"
  },
  "Лимит комментариев проекта: 500 открытых и 5000 записей истории": {
    "en": "Project comment limit: 500 open comments and 5,000 history entries",
    "es": "Límite de comentarios por proyecto: 500 abiertos y 5000 registros de historial"
  },
  "Проект удалён; сначала восстановите его": {
    "en": "The project has been deleted; restore it first",
    "es": "El proyecto se ha eliminado; restáuralo primero"
  },
  "Исчерпан счётчик комментария": {
    "en": "The comment revision counter is exhausted",
    "es": "Se ha agotado el contador de revisiones del comentario"
  },
  "Область комментария нельзя менять неявно": {
    "en": "A comment's scope cannot be changed implicitly",
    "es": "El ámbito de un comentario no se puede cambiar de forma implícita"
  },
  "Пакет ревью не найден": {
    "en": "The review packet was not found",
    "es": "No se ha encontrado el paquete de revisión"
  },
  "Лимит истории: 5000 пакетов ревью на проект": {
    "en": "History limit: 5,000 review packets per project",
    "es": "Límite de historial: 5000 paquetes de revisión por proyecto"
  },
  "Статус пакета уже изменился. Обновите данные": {
    "en": "The packet status has changed. Refresh the data",
    "es": "El estado del paquete ha cambiado. Actualiza los datos"
  },
  "Пакет не найден": {
    "en": "The packet was not found",
    "es": "No se ha encontrado el paquete"
  },
  "Пакет уже изменился": {
    "en": "The packet has changed",
    "es": "El paquete ha cambiado"
  },
  "Файл пакета уже задан": {
    "en": "The packet file has already been set",
    "es": "El archivo del paquete ya está definido"
  },
  "Сначала вставьте пакет в CLI": {
    "en": "Insert the packet into the CLI first",
    "es": "Inserta primero el paquete en la CLI"
  },
  "Подтверждение коммита уже завершено или параметры изменились": {
    "en": "Commit confirmation has already finished or its parameters have changed",
    "es": "La confirmación del commit ya ha finalizado o sus parámetros han cambiado"
  },
  "Первое подтверждение коммита должно иметь статус started": {
    "en": "The initial commit confirmation must have status started",
    "es": "La confirmación inicial del commit debe tener el estado started"
  },
  "Сессия не найдена": {
    "en": "The session was not found",
    "es": "No se ha encontrado la sesión"
  },
  "Идентификатор проекта нельзя менять": {
    "en": "The project ID cannot be changed",
    "es": "No se puede cambiar el identificador del proyecto"
  },
  "Проект не найден": {
    "en": "The project was not found",
    "es": "No se ha encontrado el proyecto"
  },
  "Проект уже изменён. Обновите данные перед сохранением": {
    "en": "The project has changed. Refresh the data before saving",
    "es": "El proyecto ha cambiado. Actualiza los datos antes de guardar"
  },
  "Исчерпан счётчик настроек запуска": {
    "en": "The launch settings revision counter is exhausted",
    "es": "Se ha agotado el contador de revisiones de los ajustes de inicio"
  },
  "Проект уже изменён. Обновите данные перед удалением": {
    "en": "The project has changed. Refresh the data before deleting",
    "es": "El proyecto ha cambiado. Actualiza los datos antes de eliminarlo"
  },
  "Лимит прототипа: 100 проектов. Удалите другой проект перед восстановлением": {
    "en": "Prototype limit: 100 projects. Delete another project before restoring this one",
    "es": "Límite del prototipo: 100 proyectos. Elimina otro proyecto antes de restaurar este"
  },
  "Сессия или проект не найдены": {
    "en": "The session or project was not found",
    "es": "No se ha encontrado la sesión o el proyecto"
  },
  "Сначала восстановите проект сессии": {
    "en": "Restore the session's project first",
    "es": "Restaura primero el proyecto de la sesión"
  },
  "Работающую сессию нельзя удалить. Сначала явно остановите её": {
    "en": "A running session cannot be deleted. Explicitly stop it first",
    "es": "No se puede eliminar una sesión en marcha. Detenla expresamente primero"
  },
  "Сначала восстановите сессию и её проект": {
    "en": "Restore the session and its project first",
    "es": "Restaura primero la sesión y su proyecto"
  },
  "Имя сессии уже изменилось. Обновите данные перед сохранением": {
    "en": "The session name has changed. Refresh the data before saving",
    "es": "El nombre de la sesión ha cambiado. Actualiza los datos antes de guardar"
  },
  "Состояние сессии изменилось": {
    "en": "The session state has changed",
    "es": "El estado de la sesión ha cambiado"
  },
  "Восстановление не запускает и не меняет работающую сессию": {
    "en": "Restoring does not start or modify a running session",
    "es": "La restauración no inicia ni modifica una sesión en marcha"
  },
  "Вложения доступны только для работающей сессии видимого проекта": {
    "en": "Attachments are available only for a running session in a visible project",
    "es": "Los adjuntos solo están disponibles para una sesión en marcha de un proyecto visible"
  },
  "Лимит сессии: 50 вложений и 100 MiB загруженных файлов": {
    "en": "Session limit: 50 attachments and 100 MiB of uploaded files",
    "es": "Límite por sesión: 50 adjuntos y 100 MiB de archivos subidos"
  },
  "Daemon перезапущен; CLI автоматически не возобновляется": {
    "en": "The local service restarted; the CLI does not resume automatically",
    "es": "El servicio local se ha reiniciado; la CLI no se reanuda automáticamente"
  },
  "Настройки запуска уже изменены. Обновите данные перед сохранением": {
    "en": "Launch settings have changed. Refresh the data before saving",
    "es": "Los ajustes de inicio han cambiado. Actualiza los datos antes de guardar"
  },
  "Профиль агента недоступен": {
    "en": "The agent profile is unavailable",
    "es": "El perfil del agente no está disponible"
  },
  "Общие настройки агента не содержат проект или папку": {
    "en": "General agent settings must not include a project or folder",
    "es": "Los ajustes generales del agente no pueden incluir un proyecto ni una carpeta"
  },
  "Дополнительные папки сохраняются только для конкретного проекта": {
    "en": "Additional folders can be saved only for a specific project",
    "es": "Las carpetas adicionales solo se pueden guardar para un proyecto concreto"
  },
  "Проект настроек не совпадает": {
    "en": "The settings project does not match",
    "es": "El proyecto de los ajustes no coincide"
  },
  "У проекта нет рабочей папки": {
    "en": "The project has no working folder",
    "es": "El proyecto no tiene carpeta de trabajo"
  },
  "Рабочая папка больше не входит в проект": {
    "en": "The working folder is no longer part of the project",
    "es": "La carpeta de trabajo ya no forma parte del proyecto"
  },
  "Дополнительные папки не должны повторяться или совпадать с рабочей папкой": {
    "en": "Additional folders must not be duplicated or match the working folder",
    "es": "Las carpetas adicionales no pueden estar duplicadas ni coincidir con la carpeta de trabajo"
  },
  "Дополнительная папка больше не входит в проект": {
    "en": "An additional folder is no longer part of the project",
    "es": "Una carpeta adicional ya no forma parte del proyecto"
  },
  "Папка не принадлежит проекту": {
    "en": "The folder does not belong to the project",
    "es": "La carpeta no pertenece al proyecto"
  },
  "Папка проекта недоступна": {
    "en": "The project folder is unavailable",
    "es": "La carpeta del proyecto no está disponible"
  },
  "Путь проекта изменился; добавьте проект заново": {
    "en": "The project path has changed; add the project again",
    "es": "La ruta del proyecto ha cambiado; vuelve a añadir el proyecto"
  },
  "Сессия удалена; сначала восстановите её": {
    "en": "The session has been deleted; restore it first",
    "es": "La sesión se ha eliminado; restáurala primero"
  },
  "Проект сессии удалён; сначала восстановите его": {
    "en": "The session's project has been deleted; restore it first",
    "es": "El proyecto de la sesión se ha eliminado; restáuralo primero"
  },
  "Исполнение недоступно: сессия завершена или daemon перезапущен": {
    "en": "Execution is unavailable: the session ended or the local service restarted",
    "es": "La ejecución no está disponible: la sesión ha finalizado o el servicio local se ha reiniciado"
  },
  "Недопустимый Host или Origin": {
    "en": "Invalid Host or Origin",
    "es": "Host u Origin no válido"
  },
  "Требуется токен локального приложения": {
    "en": "A local application token is required",
    "es": "Se requiere un token de la aplicación local"
  },
  "Ошибка локальной задачи": {
    "en": "Local operation failed",
    "es": "La operación local ha fallado"
  },
  "Укажите абсолютный путь к папке": {
    "en": "Enter an absolute folder path",
    "es": "Introduce una ruta absoluta de carpeta"
  },
  "Путь должен указывать на папку": {
    "en": "The path must point to a folder",
    "es": "La ruta debe corresponder a una carpeta"
  },
  "Путь должен поддерживать UTF-8": {
    "en": "The path must be valid UTF-8",
    "es": "La ruta debe ser válida en UTF-8"
  },
  "Папка указана дважды": {
    "en": "The folder is listed twice",
    "es": "La carpeta está duplicada"
  },
  "Нужно название проекта и от 1 до 12 абсолютных путей": {
    "en": "A project name and 1–12 absolute paths are required",
    "es": "Se requieren un nombre de proyecto y entre 1 y 12 rutas absolutas"
  },
  "Лимит прототипа: 100 проектов": {
    "en": "Prototype limit: 100 projects",
    "es": "Límite del prototipo: 100 proyectos"
  },
  "Снимок относится к другому проекту": {
    "en": "The snapshot belongs to another project",
    "es": "La captura pertenece a otro proyecto"
  },
  "Нельзя убрать папку, которую использует работающая сессия. Сначала остановите эту сессию": {
    "en": "A folder used by a running session cannot be removed. Stop that session first",
    "es": "No se puede quitar una carpeta en uso por una sesión en marcha. Detén primero esa sesión"
  },
  "В проекте есть работающие сессии. Сначала остановите их": {
    "en": "The project has running sessions. Stop them first",
    "es": "El proyecto tiene sesiones en marcha. Detenlas primero"
  },
  "Агент не установлен или профиль недоступен": {
    "en": "The agent is not installed or its profile is unavailable",
    "es": "El agente no está instalado o su perfil no está disponible"
  },
  "Слишком много дополнительных папок": {
    "en": "Too many additional folders",
    "es": "Hay demasiadas carpetas adicionales"
  },
  "Одна физическая папка указана дважды": {
    "en": "The same physical folder is listed twice",
    "es": "La misma carpeta física está duplicada"
  },
  "Лимит прототипа: 16 работающих сессий": {
    "en": "Prototype limit: 16 running sessions",
    "es": "Límite del prototipo: 16 sesiones en marcha"
  },
  "Работающую сессию нельзя восстанавливать или переигрывать": {
    "en": "A running session cannot be restored or replayed",
    "es": "No se puede restaurar ni reproducir una sesión en marcha"
  },
  "Не удалось проверить проект работающей сессии": {
    "en": "Could not verify the running session's project",
    "es": "No se ha podido verificar el proyecto de la sesión en marcha"
  },
  "Не удалось проверить папку работающей сессии": {
    "en": "Could not verify the running session's folder",
    "es": "No se ha podido verificar la carpeta de la sesión en marcha"
  },
  "В этой рабочей папке запущена CLI-сессия. Сначала остановите её": {
    "en": "A CLI session is running in this working folder. Stop it first",
    "es": "Hay una sesión de CLI en marcha en esta carpeta de trabajo. Detenla primero"
  },
  "Папка работающей сессии недоступна. Сначала остановите её": {
    "en": "The running session's folder is unavailable. Stop the session first",
    "es": "La carpeta de la sesión en marcha no está disponible. Detén primero la sesión"
  },
  "Работающая CLI-сессия имеет доступ к родительской папке этого репозитория. Сначала остановите её": {
    "en": "A running CLI session has access to this repository's parent folder. Stop it first",
    "es": "Una sesión de CLI en marcha tiene acceso a la carpeta superior de este repositorio. Detenla primero"
  },
  "Проверка работающих сессий заняла слишком много времени. Повторите после их остановки": {
    "en": "Checking running sessions took too long. Try again after stopping them",
    "es": "La comprobación de las sesiones en marcha ha tardado demasiado. Vuelve a intentarlo después de detenerlas"
  },
  "Не удалось проверить Git-папку работающей сессии": {
    "en": "Could not verify the running session's Git directory",
    "es": "No se ha podido verificar el directorio Git de la sesión en marcha"
  },
  "Работающая CLI-сессия использует этот репозиторий, включая дополнительную папку или другой проект. Сначала остановите её": {
    "en": "A running CLI session uses this repository, including through an additional folder or another project. Stop it first",
    "es": "Una sesión de CLI en marcha usa este repositorio, incluso a través de una carpeta adicional u otro proyecto. Detenla primero"
  },
  "Не удалось прочитать папку внутри проекта": {
    "en": "Could not read the folder inside the project",
    "es": "No se ha podido leer la carpeta dentro del proyecto"
  },
  "Текстовый файл недоступен, слишком велик или выходит за границы проекта": {
    "en": "The text file is unavailable, too large or outside the project",
    "es": "El archivo de texto no está disponible, es demasiado grande o está fuera del proyecto"
  },
  "Не удалось получить Git review этой папки": {
    "en": "Could not get the Git review for this folder",
    "es": "No se ha podido obtener la revisión de Git de esta carpeta"
  },
  "Нужно замечание длиной до 10 000 байт": {
    "en": "A review note of up to 10,000 bytes is required",
    "es": "Se requiere un comentario de revisión de hasta 10 000 bytes"
  },
  "Review относится к другой папке; выберите сессию этой папки": {
    "en": "The review belongs to another folder; select a session for that folder",
    "es": "La revisión pertenece a otra carpeta; selecciona una sesión de esa carpeta"
  },
  "Изменения обновились; перечитайте diff перед отправкой замечаний": {
    "en": "The changes have been updated; reload the diff before sending review notes",
    "es": "Los cambios se han actualizado; vuelve a cargar el diff antes de enviar los comentarios"
  },
  "Имя сессии должно содержать от 1 до 120 байт без управляющих символов": {
    "en": "The session name must contain 1–120 bytes without control characters",
    "es": "El nombre de la sesión debe tener entre 1 y 120 bytes y no contener caracteres de control"
  }
} satisfies Messages
