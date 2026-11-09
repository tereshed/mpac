import type { Messages } from '../i18n-types.ts'

export default {
  "Нужно замечание длиной до 4000 байт": {
    "en": "Enter feedback of at most 4,000 bytes",
    "es": "Introduce un comentario de un máximo de 4000 bytes"
  },
  "Нужен уникальный идентификатор пакета": {
    "en": "A unique package ID is required",
    "es": "Se necesita un identificador único para el paquete"
  },
  "В пакете может быть до 32 комментариев": {
    "en": "A package can contain at most 32 comments",
    "es": "Un paquete puede contener como máximo 32 comentarios"
  },
  "Сообщение должно быть не длиннее 4000 байт": {
    "en": "The message must not exceed 4,000 bytes",
    "es": "El mensaje no debe superar los 4000 bytes"
  },
  "Выберите комментарии или напишите задачу": {
    "en": "Select comments or write a task",
    "es": "Selecciona comentarios o escribe una tarea"
  },
  "Сообщение содержит управляющие символы": {
    "en": "The message contains control characters",
    "es": "El mensaje contiene caracteres de control"
  },
  "Выберите непрерывный диапазон до 200 строк": {
    "en": "Select a continuous range of at most 200 lines",
    "es": "Selecciona un rango continuo de un máximo de 200 líneas"
  },
  "Выделенные строки больше недоступны": {
    "en": "The selected lines are no longer available",
    "es": "Las líneas seleccionadas ya no están disponibles"
  },
  "Диапазон не должен пересекать пропущенные блоки diff": {
    "en": "The range must not cross omitted diff blocks",
    "es": "El rango no debe cruzar bloques omitidos del diff"
  },
  "На выбранной стороне нет строк": {
    "en": "There are no lines on the selected side",
    "es": "No hay líneas en el lado seleccionado"
  },
  "Выделенный фрагмент слишком велик; выберите меньше строк": {
    "en": "The selected fragment is too large; select fewer lines",
    "es": "El fragmento seleccionado es demasiado grande; selecciona menos líneas"
  },
  "Общая задача: ": {
    "en": "Overall task: ",
    "es": "Tarea general: "
  },
  "Пакет превышает 512 KiB. Выберите меньше комментариев или сократите задачу": {
    "en": "The package exceeds 512 KiB. Select fewer comments or shorten the task",
    "es": "El paquete supera los 512 KiB. Selecciona menos comentarios o acorta la tarea"
  },
  "Нужен уникальный идентификатор операции коммита": {
    "en": "A unique commit operation ID is required",
    "es": "Se necesita un identificador único para la operación de commit"
  },
  "Идентификатор коммита уже использован с другими параметрами": {
    "en": "This commit ID has already been used with different parameters",
    "es": "Este identificador de commit ya se ha utilizado con otros parámetros"
  },
  "Операция коммита уже запускалась. Обновите Git-состояние и проверьте последний коммит перед новым действием": {
    "en": "This commit operation has already been started. Refresh the Git status and check the latest commit before taking another action",
    "es": "Esta operación de commit ya se ha iniciado. Actualiza el estado de Git y revisa el último commit antes de realizar otra acción"
  },
  "Коммит создан, но подтверждение операции не сохранено. Проверьте показанный хеш перед повтором.": {
    "en": "The commit was created, but the operation receipt was not saved. Check the displayed hash before retrying.",
    "es": "El commit se ha creado, pero no se ha guardado el registro de confirmación. Comprueba el hash mostrado antes de volver a intentarlo."
  },
  "Подтверждение операции не сохранено. Исход коммита неизвестен; проверьте историю Git перед повтором.": {
    "en": "The operation receipt was not saved. The commit outcome is unknown; check the Git history before retrying.",
    "es": "No se ha guardado el registro de confirmación. Se desconoce el resultado del commit; revisa el historial de Git antes de volver a intentarlo."
  },
  "Ошибка задачи ревью": {
    "en": "Review task failed",
    "es": "Ha fallado la tarea de revisión"
  },
  "Проект не найден или удалён": {
    "en": "Project not found or removed",
    "es": "Proyecto no encontrado o eliminado"
  },
  "Не удалось прочитать изменения": {
    "en": "Could not read the changes",
    "es": "No se han podido leer los cambios"
  },
  "Папка недоступна": {
    "en": "Folder unavailable",
    "es": "Carpeta no disponible"
  },
  "Папка проекта заменена": {
    "en": "The project folder has been replaced",
    "es": "La carpeta del proyecto ha sido sustituida"
  },
  "Снимок изменений слишком велик": {
    "en": "The changes snapshot is too large",
    "es": "La instantánea de cambios es demasiado grande"
  },
  "Папка заменена во время проверки": {
    "en": "The folder was replaced during validation",
    "es": "La carpeta se ha sustituido durante la validación"
  },
  "Папка комментария заменена; создайте новый комментарий": {
    "en": "The comment’s folder has been replaced; create a new comment",
    "es": "La carpeta del comentario ha sido sustituida; crea un comentario nuevo"
  },
  "Изменения файла обновились; проверьте комментарий": {
    "en": "The file changes have been updated; check the comment",
    "es": "Los cambios del archivo se han actualizado; revisa el comentario"
  },
  "Выделенные строки файла изменились; создайте новый комментарий": {
    "en": "The selected file lines have changed; create a new comment",
    "es": "Las líneas seleccionadas del archivo han cambiado; crea un comentario nuevo"
  },
  "Файл изменился; проверьте комментарий к файлу": {
    "en": "The file has changed; check the file comment",
    "es": "El archivo ha cambiado; revisa el comentario sobre el archivo"
  },
  "Снимок слишком велик; уменьшите изменения перед комментированием": {
    "en": "The snapshot is too large; reduce the changes before commenting",
    "es": "La instantánea es demasiado grande; reduce los cambios antes de añadir comentarios"
  },
  "Изменения обновились; перечитайте diff": {
    "en": "The changes have been updated; read the diff again",
    "es": "Los cambios se han actualizado; vuelve a leer el diff"
  },
  "Файл не входит в текущий снимок изменений": {
    "en": "The file is not in the current changes snapshot",
    "es": "El archivo no está en la instantánea de cambios actual"
  },
  "Diff файла недоступен; оставьте комментарий к исходнику": {
    "en": "The file diff is unavailable; comment on the source instead",
    "es": "El diff del archivo no está disponible; añade el comentario al código fuente"
  },
  "Блок изменений больше недоступен": {
    "en": "The change block is no longer available",
    "es": "El bloque de cambios ya no está disponible"
  },
  "Файл изменился; перечитайте исходник": {
    "en": "The file has changed; read the source again",
    "es": "El archivo ha cambiado; vuelve a leer el código fuente"
  },
  "Исходник содержит только новые строки": {
    "en": "The source contains only new lines",
    "es": "El código fuente solo contiene líneas nuevas"
  },
  "Папка заменена во время сохранения комментария": {
    "en": "The folder was replaced while saving the comment",
    "es": "La carpeta se ha sustituido mientras se guardaba el comentario"
  },
  "У выбранной сессии нет доступа ко всем папкам пакета": {
    "en": "The selected session cannot access all folders in the package",
    "es": "La sesión seleccionada no tiene acceso a todas las carpetas del paquete"
  },
  "Проверка пакета заняла слишком много времени; выберите меньше папок": {
    "en": "Package validation took too long; select fewer folders",
    "es": "La validación del paquete ha tardado demasiado; selecciona menos carpetas"
  },
  "Идентификатор пакета уже использован для другой задачи": {
    "en": "This package ID has already been used for another task",
    "es": "Este identificador de paquete ya se ha utilizado para otra tarea"
  },
  "Выберите работающую сессию этого проекта": {
    "en": "Select a running session in this project",
    "es": "Selecciona una sesión en ejecución en este proyecto"
  },
  "Результат прошлой вставки неизвестен. Проверьте терминал; повторная вставка заблокирована": {
    "en": "The previous insertion outcome is unknown. Check the terminal; reinsertion is blocked",
    "es": "Se desconoce el resultado de la inserción anterior. Revisa el terminal; la reinserción está bloqueada"
  },
  "Пакет относится к другому проекту": {
    "en": "This package belongs to another project",
    "es": "Este paquete pertenece a otro proyecto"
  },
  "Файл пакета вышел за пределы папки сессии": {
    "en": "The package file is outside the session folder",
    "es": "El archivo del paquete está fuera de la carpeta de la sesión"
  },
  "Недопустимый путь файла пакета": {
    "en": "Invalid package file path",
    "es": "La ruta del archivo del paquete no es válida"
  },
  "Файл пакета изменён; повторная вставка отменена": {
    "en": "The package file has changed; reinsertion was cancelled",
    "es": "El archivo del paquete ha cambiado; se ha cancelado la reinserción"
  },
  "Лимит вложений сессии: 50 файлов и 100 MiB": {
    "en": "Session attachment limit: 50 files and 100 MiB",
    "es": "Límite de adjuntos de la sesión: 50 archivos y 100 MiB"
  },
  "Путь файла содержит управляющие символы; пакет не вставлен": {
    "en": "The file path contains control characters; the package was not inserted",
    "es": "La ruta del archivo contiene caracteres de control; el paquete no se ha insertado"
  },
  "Черновик мог быть вставлен. Проверьте терминал; повторная вставка заблокирована": {
    "en": "The draft may have been inserted. Check the terminal; reinsertion is blocked",
    "es": "Es posible que el borrador se haya insertado. Revisa el terminal; la reinserción está bloqueada"
  },
  "Подтвердите, что передали пакет CLI": {
    "en": "Confirm that you sent the package to the CLI",
    "es": "Confirma que has enviado el paquete a la CLI"
  }
} satisfies Messages
