# Julia twin of bench_slerp.lu — immutable value-type quaternions.

struct Quat
    w::Float64
    x::Float64
    y::Float64
    z::Float64
end

dotq(a, b) = a.w * b.w + a.x * b.x + a.y * b.y + a.z * b.z
normq(q) = sqrt(dotq(q, q))
scale(q, s) = Quat(q.w * s, q.x * s, q.y * s, q.z * s)
addq(a, b) = Quat(a.w + b.w, a.x + b.x, a.y + b.y, a.z + b.z)
normalize(q) = scale(q, 1.0 / normq(q))

function slerp(a, b, t)
    d = dotq(a, b)
    bb = b
    if d < 0.0
        d = -d
        bb = scale(b, -1.0)
    end
    wa, wb = 1.0 - t, t
    if d < 0.9995
        theta = acos(d)
        sine = sin(theta)
        wa = sin((1.0 - t) * theta) / sine
        wb = sin(t * theta) / sine
    end
    normalize(addq(scale(a, wa), scale(bb, wb)))
end

a = normalize(Quat(1.0, 2.0, 3.0, 4.0))
b = normalize(Quat(4.0, 3.0, 2.0, 1.0))
n = 2_000_000
acc = 0.0
for i in 0:n-1
    global acc += normq(slerp(a, b, Float64(i) / Float64(n)))
end
println("acc: ", acc)
