## REST API
### GET /:id
Returns the contents of selected paste (assuming paste exists, otherwise returns 404)

| Name          | Arg   | Type     | Description                                |
| ------------- | :---: | :------: | :----------------------------------------: |
| id            | path  | string   | Unique identifier of the paste             |
| lang          | query | string   | Language (used by the UI, i.e. "markdown") |

### GET /raw/:id
Returns the contents of the selected paste with HTTP `text/plain` header

| Name          | Arg   | Type     | Description                                |
| ------------- | :---: | :------: | :----------------------------------------: |
| id            | path  | string   | Unique identifier of the paste             |

### GET /download/:id
Returns the contents of selected paste with HTTP `application/octet-stream` header

| Name          | Arg   | Type     | Description                                |
| ------------- | :---: | :------: | :----------------------------------------: |
| id            | path  | string   | Unique identifier of the paste             |

### GET /static/:resource
Returns static resources, such as javascript or css files compiled in the binary

| Name          | Arg   | Type     | Description                                |
| ------------- | :---: | :------: | :----------------------------------------: |
| resource      | path  | string   | Resource name (ie. `custom.js`)            |

### POST /
Creates new paste, where input data is expected to be of type:
* `application/x-www-form`
* `application/octet-stream`

| Name          | Arg   | Type     | Description                                |
| ------------- | :---: | :------: | :----------------------------------------: |
| lang          | query | string   | Language (used by the UI, i.e. "markdown") |
| ttl           | query | int      | Expiration time in seconds                 |
| burn          | query | boolean  | Whether to delete the paste after reading  |
| encrypted     | query | boolean  | Used by UI to display "decrypt" modal box  |

### DELETE /:id
Deletes the selected paste from the local database

| Name          | Arg   | Type     | Description                                |
| ------------- | :---: | :------: | :----------------------------------------: |
| id            | path  | string   | Unique identifier of the paste             |
