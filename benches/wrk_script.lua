wrk.method = "GET"
wrk.path = "/"

wrk.headers["Host"] = "example.com"
wrk.headers["Authorization"] = "Basic cmFscGg6"

request = function()
    return wrk.format(nil)
end

response = function(status, headers, body)
end

done = function(summary, latency, requests)
    io.stderr:write(string.format(
        "\nSUMMARY: rps=%.2f, latency_mean=%.2fms, latency_stdev=%.2fms, latency_50th=%.2fms, latency_75th=%.2fms, latency_90th=%.2fms, latency_99th=%.2fms, total_errors=%d\n",
        summary.requests / (summary.duration / 1000000),
        latency.mean / 1000,
        latency.stdev / 1000,
        latency:percentile(50) / 1000,
        latency:percentile(75) / 1000,
        latency:percentile(90) / 1000,
        latency:percentile(99) / 1000,
        summary.errors.connect +
        summary.errors.read +
        summary.errors.write +
        summary.errors.status +
        summary.errors.timeout
    ))
end
