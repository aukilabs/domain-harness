import Foundation

var value = SemanticVolume(id: UUID().uuidString, name: "Kitchen", semantic_type: "room",
    position: [1, 0, 2], dimensions: [2, 3, 4], yaw: 0.4)
precondition(value.valid)
let data = try JSONEncoder().encode(value)
let decoded = try JSONDecoder().decode(SemanticVolume.self, from: data)
precondition(decoded == value)
for invalid: Float in [0, -1, 41, .infinity, .nan] {
    var candidate = value; candidate.dimensions[0] = invalid
    precondition(!candidate.valid)
}
value.position = []; precondition(!value.valid)
value.position = [1, 0, 2]; value.name = "\n"; precondition(!value.valid)
print("Semantic volume model tests passed")
