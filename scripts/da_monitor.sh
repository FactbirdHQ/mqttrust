#! /bin/bash
#
# This script is written in bash in conjunction with the AWS CLI so
# you can be aware of the APIs in use in a simplified way and optimize
# the run in the programming language of your choice.
#

STATUS_PASS=PASS
STATUS_FAIL=FAIL
STATUS_RUNNING=RUNNING
STATUS_PENDING=PENDING
STATUS_STOPPING=STOPPING
STATUS_STOPPED=STOPPED
STATUS_PASS_WITH_WARNINGS=PASS_WITH_WARNINGS
STATUS_ERROR=ERROR

# The suite definition ID should be set in the AWS CodeBuild environment.
suite_definition_id=$1

# The suite should be run prior to this script being invoked.
suite_run_id=$2

# The PID of the binary being tested.
pid=$3

STATUS_FILE=/tmp/myapp-run-$$.status
IN_PROGRESS=1
MONITOR_STATUS=0
function report_status {

    number_groups=$(jq -r ".testResult.groups | length" ${STATUS_FILE})

    echo NUMBER TEST GROUPS: ${number_groups}

    for gn in $(seq 0 $((number_groups-1))); do
        number_tests=$(jq -r ".testResult.groups[$gn].tests | length" ${STATUS_FILE})
        echo GROUP $((gn+1)) NUMBER OF TESTS: ${number_tests}
        
        for tcn in $(seq 0 $((number_tests-1))); do
            tcname=$(jq -r ".testResult.groups[$gn].tests[$tcn].testCaseDefinitionName" ${STATUS_FILE})
            tcstatus=$(jq -r ".testResult.groups[$gn].tests[$tcn].status" ${STATUS_FILE})
            echo ${tcname} ${tcstatus}
        done
    done
}

while test ${IN_PROGRESS} == 1; do
    # Fetch the current status and stash in /tmp so we can use it throughout the status fetch process.

    aws iotdeviceadvisor get-suite-run \
        --suite-definition-id ${suite_definition_id} \
        --suite-run-id ${suite_run_id} --output json > ${STATUS_FILE}
    
    # Identify the overall test status.  If FAIL or PASS, emit the status
    # and exit here with the related error code (PASS=0, FAIL=1).
    # Otherwise continue and provide overall test group and test case
    # status.

    overall_status=$(jq -r ".status" ${STATUS_FILE})
    
    echo OVERALL STATUS: ${overall_status}

    report_status
    
    if test x"${overall_status}" == x${STATUS_FAIL}; then
        MONITOR_STATUS=1
        IN_PROGRESS=0
    elif test x"${overall_status}" == x${STATUS_PASS}; then
        MONITOR_STATUS=0
        IN_PROGRESS=0
    elif test x"${overall_status}" == x${STATUS_STOPPING}; then
        MONITOR_STATUS=1
        IN_PROGRESS=0
    elif test x"${overall_status}" == x${STATUS_STOPPED}; then
        MONITOR_STATUS=1
        IN_PROGRESS=0
    elif { ps -p $pid > /dev/null; }; [ "$?" = 1 ]; then
        echo Binary is not running any more?

        MONITOR_STATUS=1
        IN_PROGRESS=0
    else
        echo Sleeping 10 seconds for the next status.
        sleep 10
    fi
done
rm ${STATUS_FILE}
exit ${MONITOR_STATUS}