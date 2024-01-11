CUR_PATH="$(dirname -- "${BASH_SOURCE[0]}")"            # relative
CUR_PATH="$(cd -- "$CUR_PATH" && pwd)"    # absolutized and normalized
if [[ -z "$CUR_PATH" ]] ; then
  # error; for some reason, the path is not accessible
  # to the script (e.g. permissions re-evaled after suid)
  exit 1  # fail
fi

docker run -it -p 1883:1883 -v $CUR_PATH/mosquito.conf:/mosquitto/config/mosquitto.conf eclipse-mosquitto